# 모델 사용량 계측 — 2026-10-04

일반적인 관측 기능이다. 추가 설정 없이 provider가 반환한 입력·출력·전체 토큰, cached input,
reasoning token, HTTP 소요 시간을 Run별 `usage.sqlite3`에 저장한다. `/usage`는 현재/직전 Run의
요약을 보여주고 `/usage RUN_ID`, `xgen usage RUN_ID`는 지정한 Run을 로컬에서 조회한다.
모델 프로필이나 credential이 없어도 조회할 수 있으며 network/model/tool을 호출하지 않는다.

## 계약과 저장 경계

- Core journal, Receipt, budget, model request profile과 proposal validation 계약은 변경하지 않는다.
  usage는 provider가 보고한 관측값이며 승인·완료 증거·청구 영수증이 아니다.
- 실제 HTTP attempt 뒤 adapter가 proposal을 decode한 경우, 응답을 거절한 경우, transport가 실패한
  경우를 각각 기록한다. `proposal_decoded`는 이후 Core가 proposal을 승인했다는 뜻이 아니다.
- 응답 거절·truncation도 사용량이 유효하면 포함한다. transport 실패는 usage가 미상이다.
  provider가 토큰을 썼지만 응답이 도착하지 않은 경우를 0 또는 환불로 해석하지 않는다.
- call ID를 SQLite primary key로 사용한다. 같은 call을 중복 합산하지 않으며 offline completion
  replay에서는 observer와 writable usage store를 열지 않는다. 각 call의 request digest를 함께 보존한다.
- private Run directory의 파일은 Unix에서 0600으로 만들고 symlink를 거절한다. SQLite 저장은
  synchronous FULL로 수행하며 DB는 64 MiB, 조회는 8192 records, record는 4096 bytes로 제한한다.
- 기록은 journal과 별도 transaction이다. process crash나 저장 실패로 관측이 빠질 수 있다.
  저장 실패는 값 없는 고정 warning을 출력하며 model 재시도나 Run 결과 변경을 유발하지 않는다.
  reserved calls와 관측 수의 차이를 표시하고 불완전한 합계를 `partial`로 구분한다.
- prompt, response content, reasoning content, argv, 경로, 환경값, endpoint, API key는 저장하지 않는다.

## 토큰 정규화

`prompt_tokens`, `completion_tokens`, `total_tokens`를 음이 아닌 정수로 읽고 전체 = 입력 + 출력을
검사한다. OpenAI 호환 `prompt_tokens_details.cached_tokens`와 DeepSeek의
`prompt_cache_hit_tokens`/`prompt_cache_miss_tokens`를 지원한다. 두 cache hit 필드가 같이 있으면
동일해야 하며 한 번만 계산한다. cached input은 입력의 일부, reasoning은 출력의 일부로 취급한다.

누락과 명시적인 0을 구분한다. 음수, overflow, 합계 불일치, cache/reasoning의 전체 초과,
중복 JSON key와 잘못된 타입은 미상으로 남긴다. usage 오류 때문에 유효한 proposal을 거절하지 않는다.
이전 binary의 Run에는 관측 기록이 없어 소급 추정하지 않는다. `tokenSubtotal`의 입력·출력·전체는
알려진 호출의 소계이며 `completeTokenUsage`와 coverage를 함께 읽어야 한다.

## 선택적인 비용 추정

기본 출력의 `costEstimate`는 null이다. USD/백만 토큰 단가 세 개를 명시할 때만 추정한다.
아래 숫자는 계산 예시이며 실제 provider 가격이 아니다.

```bash
xgen usage RUN_ID --input-price 1 --cached-input-price 0.1 --output-price 2
```

각 호출에서 `(입력 - cached 입력) × 입력 단가 + cached 입력 × cache 단가 + 출력 × 출력 단가`를
백만으로 나눈다. reasoning은 출력에 이미 포함되므로 다시 더하지 않는다. 단가는 소수점 9자리까지
고정 소수점으로 읽고 결과는 nano-USD 단위로 내림한다. float, 음수, NaN, 무한대는 받지 않는다.
입력·출력·cache가 모두 알려진 호출만 가격을 계산하며 `pricedCalls`, `partial`과 사용한 단가를 출력한다.
세금, 할인, 환율, 청구 조정은 반영하지 않는다. 가격표를 모델명이나 endpoint에서 추측하지 않는다.

## 평가와 검증

최종 응답 평가 도구는 같은 `xgen usage` 조회 결과를 저장한다. 기존 binary에서 command가 없으면
usage는 null이며 과거 RRSI 결과를 새 관측으로 바꾸지 않는다.

- 서로 다른 OpenAI 호환 cache 형식과 DeepSeek native 형식으로 실제 CLI를 실행했다.
  read → invalid final response → 별도 process resume을 거쳐 세 호출 중 두 호출의 350 tokens를 보존했다.
  usage가 없는 마지막 호출은 미상이며 부분 합계로 표시했다.
- `/usage`, offline JSON 조회, 단가를 준 비용 추정, 완료 replay의 byte 보존과 중복 미집계를 확인했다.
- missing/zero 구분, 잘못된 합계·cache·reasoning·overflow, 저장 reopen, 중복 call ID,
  sidecar symlink 거절과 가격 계산을 API 없는 테스트로 검증한다.

필드 근거: [DeepSeek Chat Completions API](https://api-docs.deepseek.com/api/create-chat-completion/),
[DeepSeek Context Caching](https://api-docs.deepseek.com/guides/kv_cache/).

## 실제 DeepSeek 확인

설치된 release binary와 저장된 `deepseek-flash` 프로필로 독립적인 일반 질문과 작은 README 읽기를
실행했다. 일반 질문은 1 call, 입력 4277·출력 60·cached input 3584 tokens였고, 파일 읽기는 2 calls,
입력 9133·출력 122·cached input 7552 tokens였다. 두 Run 모두 관측 coverage가 완전했다.
원본 private 기록은 `/tmp/xgen-usage-smoke-apie4kmg`에 남겼다. 두 실행의 관측이며 성능 개선이나
일반적인 비용 수준을 주장하지 않는다. 실제 단가를 지정하지 않아 금액은 계산하지 않았다.

최종 검증: Rust workspace 635 passed, 0 failed, 4 ignored; Python 평가 테스트 17개 통과.
workspace Clippy `-D warnings`, fmt, 공개 문서·release workflow 검사와 release build도 통과했다.

## macOS 경로 호환성 보완

첫 플랫폼 CI에서 Intel·Apple Silicon 모두 SQLite NOFOLLOW가 정상적인 상위 경로 별칭까지 거절해
usage store 생성과 조회가 실패했다. SQLite에 전달하기 전에 부모 경로만 표준 canonicalization으로
정규화하고, leaf 파일의 symlink 검사와 NOFOLLOW는 유지한다. OS 이름이나 특정 경로를 조건으로
분기하지 않는다. 직접 부모 별칭과 그 아래 nested directory 두 경우를 검증했으며 기존 leaf symlink
거절도 유지했다. 로컬 사용량 단위 테스트 5개, 거절→재개→조회 integration과 Clippy가 통과했다.
macOS 실제 통과 여부는 수정 HEAD의 플랫폼 CI로 확인한다.
