# 빌드 간 모델 요청 비교

갱신: 2026-11-27. 범용 개발 도구이며 runtime·protocol·journal은 바꾸지 않는다.

## 왜 필요한가

모델에 가는 요청이 바뀌는 변경은 사용자 확인이 필요하고, 요청 앞부분이 바뀌면 provider prompt cache가
그 지점부터 맞지 않는다. 지금까지는 이 여부를 PR마다 사람이 코드를 읽고 판단했다. 옵션별 단위 테스트
(`xgen-provider-openai`의 기존 요청 바이트 유지 검사)는 있지만, 세션 전체에서 모델이 받는 요청이 빌드
사이에 바뀌었는지는 검사하지 않았다.

`scripts/diff-model-requests.py`는 두 xgen binary를 같은 대본 세션으로 돌려 모델 요청을 비교한다.
Anthropic의 Messages API 이전 글(2026-11-25)이 쓴 방식, 즉 모델은 한 번만 정해 두고(대본 응답) 그 앞뒤
단계를 바이트 단위로 맞추는 방식을 xgen 빌드 비교에 옮긴 것이다.

## 사용법

```sh
# main과 현재 branch binary를 각각 빌드한 뒤
python3 scripts/diff-model-requests.py --base /path/to/main/xgen --head target/debug/xgen
python3 scripts/diff-model-requests.py --base ... --head ... --case headless-run-read --json report.json
```

종료 코드: 0 일치, 1 차이 있음, 2 사례 실행 실패(같은 빌드의 두 번 실행이 호출 수·종료 코드에서 다를 때 포함).

## 동작

- 사례는 [`scripts/fixtures/model-request-diff/cases.json`](../../scripts/fixtures/model-request-diff/cases.json)에
  있다. 작업 파일, CLI 인자, stdin, 대본 응답(`plan`·`completion`·`response`)으로 정의한다. 상태를 공유하는
  여러 process는 `sessions`로 적고, 뒤 session의 `{run_id}`는 앞 session이 `XGEN_STARTED`로 알린 Run이다.
  현재 8개: trust + 프로젝트 `AGENTS.md` + 읽기, 비trust 권한 허용 + 쓰기·실패 명령, 대화 두 턴, headless
  `xgen run`, headless 승인 대기 후 별도 process `xgen resume`, 대화형 읽기 거절 후 `/resume` 승인,
  `run --capability` 범위, 잘못된 완료 응답 거절 후 재질문. 대본 응답 `raw`로 잘못된 제안도 줄 수 있다.
- 로컬 fake model이 두 빌드에 같은 응답을 준다. 대본이 끝나면 completion으로 끝낸다. base 빌드가 대본을
  다 쓰지 않으면 경고한다(사례가 더 이상 의도한 경로를 타지 않는다는 뜻).
- 사례는 매번 같은 임시 경로에서 다시 만든다. 빌드마다 두 번씩 돌려, 같은 빌드에서도 달라지는 필드
  (run/step id, 그 위의 digest)를 noise로 본다. noise 필드는 16자 이상 hex와 loopback port를 가린 모양만
  비교한다. 필드 이름 목록을 코드에 두지 않는다.
- 요청 body와 그 안의 JSON 문자열(message content)을 순서를 유지한 채 펼친다. 값, 배열 길이, 객체 key
  순서를 따로 비교한다. 같은 값이라도 key 순서가 바뀌면 cache 접두부가 바뀌므로 차이다.
- 차이마다 지문(배열 index를 `*`로 바꾼 위치, 종류, 길이 변화)을 붙여 묶는다. 한 변경이 모든 사례·호출에
  남긴 차이가 한 묶음이 된다.
- 요청마다 hex·port를 가린 body에서 처음 달라지는 byte를 보인다. prompt cache가 맞지 않기 시작하는 곳이다.

## 검증 (과거 PR의 부모와 merge commit 빌드)

도구는 현재 main 빌드끼리 비교(일치)로만 맞춘 뒤, 아래 5쌍에 처음 돌렸다.

| PR | 기대 | 결과 |
|----|------|------|
| #131 지침 파일 | trust 사례의 goal에만 지침 추가, headless는 그대로(ADR-0075) | 차이 2개 1묶음: `planningContext/goal` +338자, trust 사례 2회 호출만 |
| #124 cache 순서 | planning context 순서 변경 | 차이 18개 2묶음: `profileVersion` v2→v3, `planningContext` key 순서(`toolOutputs`가 `steps` 앞) |
| #144 capability 범위 | 기본(범위 미지정) 요청 유지(ADR-0087) | 일치 |
| #146 parameter style | 기본 compatible 요청 유지(ADR-0086) | 일치 |
| #147 TUI 다시 그리기 | 요청 무관 | 일치 |

- 5쌍 모두 ADR의 기대와 같았다. 오탐 0, 누락 0. 다만 사례 4개가 다루는 경로 안에서의 결과다.
- #131의 첫 차이 byte는 약 28.8 KB 요청의 26,244번째였다. #124 순서 덕분에 goal이 뒤쪽에 있어서 지침
  추가가 앞쪽 약 91%의 cache를 깨지 않는다. ADR-0075의 "남은 일"(지침 비용)에 대한 근거 하나다.
- #124는 interactive 요청의 3,150번째 byte(headless 2,197)부터 달랐다. 의도된 cache 순서 변경 PR의 비용이
  한 번 cache를 비우는 정도라는 것을 PR 전에 숫자로 볼 수 있다.
- 단위 테스트 6개(`scripts/tests/test_model_request_diff.py`): 같은 빌드 일치와 noise, 필드 변경의 지문 묶음,
  key 순서 차이, noise 필드의 모양 비교, 펼치기, 반복 불가능한 빌드의 오류 처리. 실제 xgen 대신 같은
  wire 형식을 보내는 stand-in을 쓴다.

## 사례 추가 검증 (2026-11-27)

resume·거절·범위·응답 거절 재질문 사례 4개를 더한 뒤 처음 돌린 4쌍:

| 변경 | 기대 | 결과 |
|------|------|------|
| ADR-0067 (`b75ffe5`) | idempotent Step의 `arguments`를 planning context에서 뺀다 | 차이 14개 3묶음: `steps/*/arguments` 제거. 새 resume 사례 2개에서도 같은 지문 |
| #144 | 범위 사례만 달라진다(이전 빌드는 `--capability`를 모름) | `headless-capability-scope`만 `#calls 0->2`, `#exit 2->0`, 나머지 6개 일치 |
| #149 안내 문구 | 요청 무관 | 일치 |
| ADR-0064 (`1535af8`) | 거절된 응답 뒤 재질문에만 거절 이유를 넣는다 | 재시도 사례의 3번째 호출에만 `previousResponseRejection`(reason, meaning) 추가, 나머지 7개 일치 |

- 처음에는 key가 빠진 객체가 "key 목록 변경"으로도 한 번 더 잡혀 같은 변경이 두 묶음으로 나왔다. 이제 공통
  key의 순서가 바뀐 경우만 순서 차이로 보고, 추가·제거는 각 위치에서만 보고한다. 객체 전체가 추가·제거되면
  그 필드들만 보고한다.
- base 빌드가 사례를 못 돌리면(예: 없는 option) 첫 오류 줄과 함께 경고한다.

## 한계

- 사례가 지나가는 경로만 본다. HTTP 도구, web search, PTY terminal 경로는 아직 사례가 없다. web search·HTTP는 별도 backend가 필요하다.
- 모델 품질이 아니라 요청 바이트만 본다. 요청이 바뀐 것이 좋은지 나쁜지는 사람이 판단한다.
- CI에서는 돌리지 않는다. main binary를 따로 빌드해야 해서 PR 시간이 늘어난다. 지금은 모델 요청에 영향이
  있을 수 있는 PR에서 로컬로 돌리고 결과를 PR 본문에 적는다.
- noise 판정은 같은 빌드 두 번 실행에 기댄다. 두 번 모두 같지만 환경마다 다른 값(예: 실행 파일 digest)은
  같은 기계에서 비교하므로 차이로 나오지 않는다. 다른 기계의 결과와는 비교하지 않는다.
