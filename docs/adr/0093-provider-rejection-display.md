# ADR-0093: Provider 거절 이유 표시

- 날짜: 2026-11-28
- 상태: 채택, 구현·로컬 검증 완료
- 범위: OpenAI 호환 HTTP provider의 4xx 응답. `xgen run`/`resume`의 거절 출력, 대화형 세션의 거절 출력,
  `xgen model check`/`setup`의 실패 출력. journal, planning context, model 요청은 바꾸지 않는다. agy 연결은 대상이 아니다.

## 근거

Provider가 요청을 4xx로 거절하면 xgen은 상태 코드만 보고 `provider_rejected`나 `request_rejected` 같은 class 하나만
보여 줬다. Run 경로에서는 401도 `provider_rejected`로 묶였다. 그래서 잘못된 key, 모델 ID 오타, 지원하지 않는
파라미터, 요청 한도, 콘텐츠 정책이 모두 같은 한 줄로 보였고, 안내 문장도 class마다 고정이라 원인과 어긋났다.
Provider는 응답 본문에 이유를 적어 보내지만 xgen은 그 본문을 읽지 않고 버렸다
([ADR-0088](0088-first-use-preview-distribution.md)의 "Provider 응답 원문을 오류에 추가하지 않는다").

## 결정

1. 4xx 응답이면 본문을 최대 16 KiB까지 읽어 `ProviderRejection{status, kind, code, message}`로 요약한다.
   널리 쓰이는 오류 형식을 읽는다.
   - `error` 객체: OpenAI 호환 API와 Anthropic
   - 한 원소 목록으로 감싼 `error`: Google
   - `error` 문자열: Ollama
   - 최상위 `message`: vLLM
   - FastAPI `detail`: 문자열 또는 검증 목록의 첫 `msg`
   형식을 모르면 상태 코드만 남긴다. HTML 같은 markup 본문은 요약하지 않는다.
2. 요약은 사람이 보는 화면 전용이다. 표시 전에 다음을 처리한다.
   - 제어 문자와 보이지 않는/방향 전환 문자를 지우고 공백을 합친다.
   - 이번 요청에 보낸 key와 `Bearer` 다음 단어를 지운다.
   - 알려진 key 접두사(`sk-`, `AIza` 등)로 시작하는 문자열을 지운다. Provider가 가려서 돌려준 key의 앞부분도 지운다.
   - 대소문자와 숫자가 섞인 24자 이상 문자열과 32자 이상 hex를 지운다.
   - message는 300자, type과 code는 64자에서 자른다.
   요약은 journal, Run 상태, usage 기록에 쓰지 않는다. 다음 모델 요청에도 넣지 않는다.
3. Run이 provider 측 거절(`provider_rejected`, `request_too_large`, `rate_limited`, `provider_limit`)로 끝나면 결과에
   요약을 붙인다. Headless는 `XGEN_REJECTED` 아래에 `provider:`와 `recovery:` 줄을, 대화형 세션은
   `Provider said:` 줄과 안내 문장을 출력한다. 안내는 상태 코드로 고른다.
   - 401/403: key 재설정
   - 404: exact model ID 확인
   - 429: 한도 대기
   - 나머지: provider 메시지를 먼저 읽고 요청 옵션 확인
4. `model check`/`setup`은 기존 `reason=`과 `recovery:` 사이에 `provider:` 줄을 넣는다. Compatibility probe가
   401/403을 받으면 `request_rejected`가 아니라 `authentication_rejected`(exit 77)로 분류한다.
5. Journal의 거절 class와 Run 결과 코드는 그대로다. 나중에 `resume`하거나 Run을 조회할 때는 이유가 다시
   보이지 않는다. Journal에 남기는 방안(B안)은 실제 사용에서 필요해지면 따로 결정한다.

## 검증

- Provider 단위 테스트: 형식 7종, markup·빈 본문, key·Bearer·hex·Google key·방향 전환 문자 제거, 가려진 key의
  앞부분 제거, 모델 ID 보존, 길이 제한.
- HTTP 계약 테스트:
  - 400 거절의 message는 표시 observer에만 전달되고 journal과 state에는 없다.
  - 401 catalog 거절은 checker에서 한 번만 꺼낼 수 있다.
- 실제 provider에 가짜 key를 보냈다.
  - 설계 사례: DeepSeek와 OpenAI.
  - 설계에 쓰지 않은 사례 9곳: Groq, Mistral, Together, Fireworks, Cerebras, DeepInfra, xAI, Anthropic,
    Google OpenAI 호환. 처음에는 Mistral(FastAPI `detail`)만 이유가 비었고, 형식을 추가한 뒤 9곳 모두
    이유가 보였다.
  - OpenAI가 돌려준 가려진 key의 앞 8자는 처음에 남아 있어서 지우도록 고쳤다.
  - xAI와 Google은 잘못된 key에 400을 돌려줘 class는 `request_rejected`지만, 메시지로 원인이 보인다.
- Headless Run(DeepSeek, xAI, Groq)과 tmux 대화형 세션(로컬 400 mock)에서 이유와 안내가 보였다. Run 상태
  폴더에는 provider 메시지가 남지 않았다.
- `scripts/diff-model-requests.py`를 rc.9 binary와 비교했다. 8개 사례 모두 모델 요청이 같았다.
