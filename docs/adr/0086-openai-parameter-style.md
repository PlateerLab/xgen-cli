# ADR-0086: OpenAI 출력 한도·sampling 파라미터 이름을 명시적 wire 옵션으로 고른다

- 날짜: 2026-11-03
- 상태: 채택, 구현·실제 OpenAI 검증 완료
- 범위: OpenAI-compatible adapter, CLI profile·preset. ADR-0042/0043의 명시적 wire 옵션 원칙을 따른다.

## 문제

OpenAI preset으로 `xgen model setup`을 하면 OpenAI의 어떤 model도 통과하지 못했다. 2026-11-03에
api.openai.com으로 직접 요청을 나눠 보내 세 가지 원인을 확인했다.

1. 추론 model(gpt-5.5, gpt-6-luna)은 `max_tokens`를 `Unsupported parameter`로 거절하고
   `max_completion_tokens`를 요구한다. `temperature: 0`도 거절한다(기본값 1만 허용). `seed`는 받는다.
   gpt-4.1은 `max_completion_tokens`와 temperature 생략을 모두 받는다.
2. preset의 `json_schema`는 OpenAI strict structured outputs에서 모든 model이 400으로 거절한다.
   strict 모드는 모든 schema node에 `type`을 요구하는데 proposal schema의 Step `arguments`는
   열려 있다. `json_schema_atomic_json`도 `arguments.jsonContent`에서 `additionalProperties: false`
   요구로 거절된다.
3. 별칭 model(`gpt-4.1`, `gpt-5.5`)은 응답 `model`에 날짜 붙은 snapshot ID
   (`gpt-4.1-2025-04-14`, `gpt-5.5-2026-04-23`)를 돌려주므로 정확한 model identity 검사에서 닫힌다.

기존 `--thinking enabled`는 temperature/seed를 빼지만 `thinking` 객체를 보내고 OpenAI는 이를
`Unknown parameter`로 거절한다. 따라서 기존 옵션 조합으로는 OpenAI 추론 model을 쓸 수 없다.

## 결정

`model setup`, `model check`, `run`, `resume`에 `--parameter-style compatible|openai`를 더한다.
환경변수는 `XGEN_OPENAI_PARAMETER_STYLE`, profile 필드는 `parameterStyle`이다. 해석 순서는 다른 wire
옵션과 같다(명시 option, 환경변수, profile, 기본값). 잘못된 값은 fail-closed다.

- `compatible`(기본): 기존 그대로 `max_tokens`와 `temperature: 0`을 보낸다(thinking 규칙 유지).
- `openai`: 같은 출력 한도를 `max_completion_tokens`로 보내고 `temperature`를 생략한다. `seed`,
  `response_format`, thinking 확장 규칙은 바꾸지 않는다.

model ID나 hostname으로 style을 추측하지 않는다. 거절 뒤 다른 style로 자동 재시도하지 않는다.
OpenAI preset만 `openai` style과 `json_object`를 기본값으로 가진다. `json_object`는 host-validated이며
server가 schema를 강제한다는 주장은 하지 않는다. 다른 preset은 바뀌지 않는다.

gpt-4.1 같은 비추론 model도 이 preset에서는 temperature 기본값(1)으로 동작한다. temperature 0이
필요하면 `--parameter-style compatible`을 명시한다.

별칭 model의 identity 검사는 완화하지 않는다. 별칭이 가리키는 snapshot이 Run 도중 바뀌는 것을 막는
검사이기 때문이다. 사용자는 catalog에 함께 나오는 날짜 붙은 snapshot ID를 선택한다.

## 재개와 저장 형식

style은 request profile digest에 들어간다. `compatible`일 때는 descriptor에 필드를 넣지 않으므로
기존 profile과 Run의 digest가 그대로다(기본 digest 상수 테스트 유지). `openai`로 바꾼 profile로
기존 Run을 재개하면 `configuration_mismatch`로 닫힌다.

`StoredProfile`은 `deny_unknown_fields`라서 새 필드는 `openai`일 때만 파일에 쓴다. style을 고르지
않은 profile 파일은 바이트 그대로이며 이전 binary도 읽는다. `openai` profile이 든 파일은 이전 binary가
읽지 못한다.

## 검증

provider 단위 테스트: openai style의 `max_completion_tokens`·temperature 생략·seed 유지, json_object와의
조합, digest 차이와 compatible 복귀 시 원래 digest, wire 이름(`compatible`/`openai`). CLI 테스트:
profile round trip, 잘못된 값 거절, compatible profile의 필드 미기록, preset 해석, probe/planner digest
일치.

실제 OpenAI(2026-11-03, 격리된 config/state, 키 미저장):

- `--provider openai` setup PASS: gpt-6-luna, gpt-5.5-2026-04-23, gpt-4.1-2025-04-14.
  별칭 gpt-5.5는 위 3번 원인으로 `chat_completions_incompatible`.
- `xgen run` 파일 수정(add 버그) 3/3 완료(4~14초). 테스트 파일 생성+README 수정 2/2 완료
  (gpt-6-luna 5 call 15초, gpt-5.5 snapshot 7 call 33초), 생성된 pytest 4 passed. 출력 한도 1024에서
  잘린 call은 없었다.

거절 이유가 `request_rejected` 한 줄로만 보여 원인을 알 수 없었던 문제와 별칭 model 안내는 이 ADR의
범위가 아니며 후속 작업으로 남긴다.
