# ADR-0090: 실험용 agy 모델 transport

- 날짜: 2026-11-23
- 상태: 실험 기능 구현. 실제 모델로 새 Rust 경로 검증은 인증 복구 뒤 수행.
- 범위: 일반 모델 연결·프로필·중단·재개. 특정 과업의 답이나 도메인 규칙은 포함하지 않는다.

## 배경

[기존 평가](../research/2026-11-23-agy-coding-baseline.md)는 공식 agy의 Gemini 제안을 xgen 도구로
실행해 20회 중19회 완료했다. [후속 확인](../research/2026-11-23-agy-controls.md)에서 usage 변환을
수정했으나 native 도구의 사전 완전 차단은 검증하지 못했다. 사용자는 이 제한을 전제로 터미널에서
선택할 수 있는 실험 기능 구현과 저렴한 Flash Low 기본값을 요청했다.

Python bridge나 별도 서버를 사용자에게 요구하지 않는다. Core planner/journal 계약은 유지한다.

## 결정

`xgen model setup --provider agy`와 대화형 provider 목록에 experimental 항목을 추가한다.
첫 지원 플랫폼은 실제 검증 기반이 있는 Linux다. macOS/Windows에서는 setup을 명시적으로 거절한다.
다른 provider의 플랫폼 지원과 저장 형식은 유지한다.

- `--agy-executable`로 지정하거나 PATH에서 찾은 공식 CLI를 canonical path와 SHA-256으로 고정한다.
  기본 model은 `gemini-3.8-flash-low`, inference timeout은60초다. 기존 agy 프로필을 갱신할 때는 지정하지
  않은 model/timeout 값을 유지한다. Catalog만 확인해 저장하며, inference 검증을 했다고 표시하지 않는다.
- xgen은 agy의 로그인 정보를 읽거나 복사·저장하지 않는다. 별도 API key가 필요하지 않다. agy의 원래 HOME과
  DBus 등 필요한 환경만 전달한다. SSH 변수·API keys·다른 host 설정은 전달하지 않는다.
- Profile의 기존 endpoint 문자열에 `agy:///absolute/path?sha256=...` descriptor를 저장한다. credential
  reference는 없다. Agy가 하나라도 포함되면 profile format2를 쓰고, API만 있는 설정은 format1을 유지한다.
  새 CLI는 둘 다 읽는다. 구버전 CLI는 format2를 읽지 못하므로 개발 빌드와 rc.8은 설정 폴더를 분리한다.
  마지막 agy 프로필을 제거하면 format1로 저장한다. HTTP 프로필의 기본값·필드는 바꾸지 않는다.
- CLI composition이 descriptor를 해석한다. 실행 파일·agent 정의·settings template·한도·환경/usage 계약의
  파생 hash를 synthetic endpoint로 만들어 기존 request-profile digest에 포함한다. 이 endpoint는 HTTP로
  호출하지 않는다. URI 해석을 Core나 OpenAI wire adapter에 넣지 않는다.
- OpenAI wire adapter에는 host transport callback을 받는 생성자를 추가한다. 기존 system prompt·request
  envelope·response/proposal/schema 검증·usage 관찰을 그대로 재사용하며, callback에는 credential을 주지
  않는다. 기존 HTTP 생성자와 endpoint/credential 보안 검사는 유지한다.
- 매 transport마다 private 임시 디렉터리에 검증한 CLI 사본과 고정 agent/settings를 만든다. 실제 사용자
  workspace는 agy의 cwd로 열지 않는다. 원본/사본 binary와 생성 파일을 전후 검사하고 workspace의 추가
  파일·symlink·settings 변경을 거절한다. 임시 디렉터리는 transport 종료 시 제거한다.
- 각 호출은 새 agy conversation으로 제안만 받는다. 실제 host 파일/process 실행·승인·Receipt·복구는
  xgen이 소유한다. Host가 Run 예산으로 논리적 호출을 제한하며 CLI 내부 retry 횟수는 알 수 없다.

## 한도·취소·실패

Prompt는 Linux에서100,000 bytes, stdout131,072 bytes, stderr32,768 bytes다. Reader channel과
capture 자체를 제한하고 stderr·provider 원문을 사용자 출력에 복사하지 않는다. Timeout·출력 초과·잘못된
identity/JSON·native tool/subagent event는 실패로 처리한다. JSON의 중복 key와 비유한값도 거절한다.
`--max-output-tokens`는 agy setup에서 거절한다. Native token cap이나 USD cap을 강제할 수 없다.

Native 활동은 관찰 후 process group을 중단한다. 도구가 이미 실행됐을 가능성을 지우지 않으며 이를
사전 차단·OS sandbox·raw model API라고 부르지 않는다. 불확정 실패를 완료나 안전한 자동 재시도로
바꾸지 않는다. Native 세부 사유는 고정 stderr code, journal은 기존 model-call Unknown 계약을 쓴다.

TUI/REPL의 취소 flag를 agy polling loop에도 연결한다. Headless SIGINT와 host SIGTERM/SIGHUP의 기존
watcher에는 일반 host cleanup hook을 추가해 owned agy process group과 private 임시 폴더도 정리한다. Callback은 signal
handler 안이 아니라 watcher thread에서 실행한다. 기존 PTY cleanup도 그대로 수행한다. Process group을
벗어난 임의의 외부 native 동작까지 되돌리거나 완전히 통제한다는 보장은 아니다.

## 재개와 사용량

미완료 Run은 기존 workspace/catalog/profile 검사에 더해 binary/policy drift를 검사한다. 원본 실행
파일이 바뀌거나 없어지면 모델 호출 전에 configuration mismatch로 거절한다. 생성된 agent/settings나
policy revision이 바뀌면 파생 request digest도 바뀐다. 이미 완료된 Run은 기존 순서대로 먼저 offline
replay하므로 binary·workspace·catalog 없이 재생할 수 있다.

`agy-uncached-input/v1`: CLI input은 cache를 제외하므로 host prompt=input+cache, host total은 여기에
output을 더한다. CLI total=input+output을 검증하며 thinking은 다시 더하지 않는다. 누락·overflow·잘못된
usage는 기존 UNKNOWN 관찰로 보존한다. 실제 USD와 provider 내부 요청 수는 미확인이다.

`xgen model logout`은 agy의 공유 로그인을 지우지 않는다. agy가 관리하는 로그인임을 안내하고, 프로필
삭제는 xgen의 non-secret 설정만 제거한다. API credential을 저장한 기존 프로필 이름에 agy를 덮어쓰는
것은 거절하고 별도 이름을 사용한다.

## 검증과 한계

합성 CLI를 실제 public setup/run/resume에 붙여 프로필·키/SSH 환경 제외·승인 전 정지·같은 Run 재개·
완료 offline replay·binary/settings drift·native/timeout/출력 초과·SIGINT 후 지연 child effect가 남지
않는 것을 검사했다. 공식 예제와 서로 다른 두 실측 usage를 unit 검사에 사용했다. 기존 HTTP 경로의
회귀를 포함한 전체 Rust 검사를 수행한다. 이 검사는 새 연결기의 실제 모델 품질 점수가 아니다.

실제 agy smoke는 catalog 단계에서 인증 필요로 중단됐다. 같은 제한 환경에서 원래 CLI와 사본을 직접
조회해도 실패했고 모델 inference는 시작하지 않았다. 기존 19/20을 새 Rust transport 실측으로 재표기하지
않는다. Delegate MCP diff 검토도 인증 오류로 이용하지 못해 직접 검토·회귀 검사를 수행했다.

배포본은 rc.8을 유지한다. macOS/Windows 실행 지원·독립 diff 검토·새 Rust 경로의 실제 모델 smoke는
후속 검증 대상이다. Native 사전 차단과 숨은 request/billing 정보는 agy 경로의 별도 개선 과제다.

최종 순차 검사: Rust808개 통과·4 ignored, Python178개 중122개 통과·56 skip, workspace clippy·fmt·
공개 문서·protocol 검사 통과. [검증 집계·source/log hash](../development/agy-provider-validation-2026-11-23.json).
초기 병렬 Python 검사1개는 완료 증거 수집에 실패했다. 정확한 원인은 확정하지 않았고, 같은 focused 검사와
바이너리 빌드 후 전체 순차 검사는 통과했다. 최초 실패 기록도 보존했다.
