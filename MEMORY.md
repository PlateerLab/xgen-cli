# xgen CLI와 연계 도구 공동 개발 메모

갱신: 2026-10-31 (KST)

## 사용자가 정한 방향

- xgen CLI는 여러 도구를 받아 사용하고, 실제 작업을 완료하는 하네스다.
- 연계 도구도 xgen CLI와 함께 계속 개발한다. xgen 쪽 연결만 개발하고 도구 자체의 개선을 놓치지 않는다.
- 사용자는 `xgen`을 실행해서 기능을 사용한다. 별도 서버 운영, Python 설치, 도구별 JSON 설정을 사용자에게 요구하지 않는 방향이다.
- 시스템 연결·로그인 후 명세나 화면 정보를 수집하고, 도구 목록과 관계를 구성해서 필요한 도구를 선택한다.
- 화면 조작에는 browser-use를 활용하는 방향이다. graph-tool-call은 명세·수집 정보를 도구 그래프로 구성하고 검색하는 데 활용한다.
- SEV 판단 모델도 함께 개발·검토한다. API 도구와 브라우저 작업을 공통 그래프에서 찾고, 실제 화면 대상은 현재 관찰을 바탕으로 선택하는 방향이다.
- 화면에는 작업 중 상태 표시와 최종 응답이 우선이다. 상세 실행 과정의 상시 노출은 필수가 아니다.

## 책임과 개발 범위

| 구성 요소 | 책임 | 별도로 추적할 개선 |
| --- | --- | --- |
| xgen CLI | 연결·인증 관리, 도구 호출 계약, 권한·승인, 실행, Receipt, 최종 응답, 사용자 경험 | 연결 흐름, 어댑터, 결과 검증, 배포·자동 실행 |
| graph-tool-call | 도구 정의 수집·정규화, 관계 그래프 구성, 관련 도구·선행 도구 검색 | 수집 지원 범위, 계약 보존, 검색 품질, 관계의 근거, 성능 |
| browser-use | 화면 관찰, 탐색·조작, 실행 결과 관찰 | 수집 결과 구조화, 동작 재현성, 화면 변화 감지, 실행 증거 |
| SEV | 제공된 상태와 후보에 대한 typed-choice 판단 | 후보 선택·거절 품질, 입력 형식, 체크포인트·평가·판단 비용 |
| 화면 정보 변환 어댑터 | 화면 관찰을 입력·출력이 있는 도구 정의로 변환 | 관찰 출처·시점, 불확실성, 선행 조건, 성공 조건 |

도메인 규칙은 설정이나 에이전트 프롬프트에 둔다. 공통 엔진과 연계 도구에 특정 업무 전용 규칙을 넣지 않는다.

### 실패 원인과 수정할 저장소의 구분

사용자 지침: xgen 통합 개발과 개별 도구 품질 개선을 구분한다. 실패가 xgen 화면에 나타난다는 이유로 xgen 엔진의 문제로 분류하지 않는다. 아래는 일반적인 판정 기준이며 실제 원인은 재현으로 확정한다.

| 관찰·확인 조건 | 개선 소유자 | xgen의 책임 |
| --- | --- | --- |
| 원본 명세에 설명·필수 입력·응답 정의가 없음 | 명세 제공 시스템·도구 정의 작성자 | 누락을 표시하고 실행 지원 여부를 알림. 업무 규칙을 추측해 보완하지 않음 |
| 원본에는 있는데 graph-tool-call 직접 변환에서도 정보가 빠짐 | graph-tool-call parser·정규화 계약 | 보존된 원본과 버전을 전달하고 호환 어댑터 제공 |
| 같은 올바른 검색어·snapshot·설정으로 직접 검색해도 정답이 top-k에 없음 | graph-tool-call 검색·색인·설정 경로 | 후보 요청·전달과 예산 관리. 검색 알고리즘을 하네스에 중복 구현하지 않음 |
| 직접 호출에는 정답이 있지만 xgen 경유에서는 누락됨 | xgen worker·어댑터·컨텍스트 전달 | 같은 요청·설정 전달, 손실 없는 결과 전달, 컨텍스트 한도 처리 |
| 올바른 후보와 필요한 계약이 모델 입력에 있는데 잘못 선택함 | 사용 중인 추론 모델 또는 SEV의 선택·프롬프트·평가 | 역할 연결·입출력 계약·예산을 관리. 선택 모델의 품질과 구분 |
| 좋은 검색어로는 성공하지만 모델이 만든 검색어로 실패함 | xgen 계획/검색 요청 구성과 해당 모델·프롬프트 | 원래 요청과 검색어를 비교하고 요청 구성 개선. 검색기만의 실패로 단정하지 않음 |
| 올바른 도구를 골랐지만 주소·인증·파라미터 변환·승인·재개·Receipt가 잘못됨 | xgen HTTP/브라우저 어댑터·호스트 실행 경로 | 실제 실행과 검증을 수정. API 자체의 장애·잘못된 명세와는 따로 판별 |
| 같은 유효한 요청을 직접 보내도 API가 실패하거나 잘못된 데이터를 반환함 | 연계 서비스·API 제공자 | 실패를 기록하고 근거 없는 완료 주장을 막음 |

관계 그래프의 잘못된 producer 후보는 graph-tool-call 과제다. 추론한 관계를 실행으로 입증된 관계처럼 사용하는 문제는 xgen 과제다. 브라우저도 같은 방식으로 관찰/동작을 실행기에서 직접 재현한 실패와 xgen의 stale-ref·재개·결과 전달 실패를 구분한다.

책임 확정은 같은 입력을 단계별로 재생해 수행한다: 원본 명세 → 정규화 결과 → 고정 검색어의 직접 검색 → xgen 경유 후보 → 모델에 실제 전달한 후보/계약 → 선택 → 직접 API/브라우저 실행 → xgen 실행·검증. 검색어·snapshot·backend 설정·top-k·후보 ID·시간/호출 비용을 비교하고 인증값·민감한 원문은 공동 기록에 넣지 않는다.

초기 검색 첫 후보 혼동과 기존 선택률 기록만으로는 소유자를 확정하지 않았다. 이후 고정 검색어의 직접→CLI→agent 분리 평가에서 검색 후보 전달 차이 0건, 큰 describe의 agent 전달 실패 14/32건을 확인했다. 이 조건의 후보 누락은 graph-tool-call 검색/색인 과제이고 큰 계약 전달·한도 오류 UX는 xgen 과제다. 초기에는 전달 실패로 모델 검색어 생성·선택 품질을 미평가했고, 이후 별도 baseline 결과는 아래에 기록했다. 자세한 범위와 수치는 아래 결과 기록을 따른다. 임베딩·reranking 추가는 새 검증 사례의 효과를 확인한 뒤 결정한다.

## 설계 원칙과 아직 미확정인 것

- 일반: graph-tool-call이 제안한 도구와 호출 관계를 xgen이 검증한다. 검색 결과를 권한 부여나 실행 성공으로 취급하지 않는다.
- 일반: 실제 호출은 xgen의 승인·실행·Receipt 경로를 통과한다. 범용 프록시를 쓰더라도 실제 도구 ID, 대상, 입력을 기록한다.
- 일반: 화면 관찰은 확정된 API 명세가 아니다. 관찰한 동작, 추론한 관계, 실행으로 확인한 결과를 구분한다.
- 일반: 저장한 그래프에는 원본 명세·관찰의 출처와 버전을 연결하고, 변경 시 갱신한다. 인증 정보는 모델 입력과 그래프에 저장하지 않는다.
- 일반: 다른 언어의 도구를 내부에서 자동 실행할 수 있다. Rust 재구현이나 MCP 사용 자체가 목표는 아니다.
- 초기 구현: 바이너리에 포함한 Python worker를 stdin/stdout JSON으로 호출하고 uv가 Python 3.12·graph-tool-call 0.47.0을 자동 준비한다(2026-10-09 0.46.0에서 갱신, 이전 버전 collection은 재import 필요). 별도 서버는 없지만 현재 개발 버전은 uv가 필요하다. uv 포함 배포·OS별 검증은 남았다.
- 구현: agent loop의 저장 collection 자동 연결·검색·정확한 명세 조회와 연결된 GET·호스트 지정 POST 조회·로컬 인증 참조. HTTP query 배열과 Run별 bearer 확장은 아래 결과를 따른다. 미구현: 승인 있는 HTTP 쓰기, browser-use 연결, 화면 정보 변환 어댑터.

## 역할별 모델 연결: 설계 후보

사용자는 추론 LLM, 판단 모델, STT 등을 xgen 하나에서 사용할 수 있게 제공하고, 향후 한 모델이 여러 역할을 수행하는 경우도 수용하자는 방향을 제안했다. 다음은 검토할 일반 설계이며 아직 구현하거나 성능을 검증하지 않았다.

- 역할과 실제 모델을 분리한다. 계획·응답, 후보 선택, 음성 인식, 필요 시 화면 해석·음성 합성을 각각 연결할 수 있다.
- 같은 모델이 여러 역할을 맡거나 서로 다른 모델이 나눠 맡을 수 있다. 엔진을 두 모델 구조나 특정 제공자에 고정하지 않는다.
- 역할별 계약은 유지한다. 계획 결과, 후보 ID 선택, 누적·최종 STT 전사, 실행 증거를 하나의 자유 텍스트 응답으로 뭉개지 않는다.
- xgen은 역할별 모델 바인딩과 호출 순서·예산·타임아웃을 관리한다. confidence가 높다는 이유로 승인·실행 검증을 생략하지 않는다.
- 모델을 바꾸거나 한 모델로 통합할 때도 각 역할의 품질과 전체 작업 완료·지연·비용을 별도로 평가한다. 통합 모델이 모든 역할에서 우수할 것이라는 가정은 하지 않는다.
- 사용자에게는 하나의 실행 명령과 기본 구성을 제공한다. 역할별 고급 설정은 필요할 때만 노출하고, STT 같은 선택 기능을 기본 텍스트 사용의 필수 의존성으로 만들지 않는다.
- 범위는 모델 연결·실행 조율이다. 모델 학습·서빙 자체는 SEV 등 해당 구성 요소가 소유한다. 사용자의 별도 서버 운영을 기본 전제로 삼지 않는다.

## 현재 확인한 결과

- `xgen tools import/search/describe/list`로 graph-tool-call 기반 발견·검색을 구현했다. agent loop는 여전히 `CapabilityRegistry`에 등록한 도구를 ID·버전 순서로 컨텍스트 한도 안에서 제공한다. `CapabilityRouter`는 지정된 exact Capability의 실행 Instance를 선택하며 자연어 검색기가 아니다. 새 workspace Run에는 검색·조회 built-in 두 개를 등록하고 collection snapshot을 manifest에 고정한다. 외부 후보별 자동 Definition/Instance admission은 아직 없고 연결된 GET의 실행은 `xgen.http/read` 프록시로 제공한다.
- [내부 검색 계약·구현](docs/development/internal-tool-search.md): 원본 계약과 snapshot digest 보존, 덮어쓰기 없는 압축 저장, 변조 검증, 제한된 worker 통신. 검색 후보는 실행 권한이 없고 효과 미분류 상태다.
- 이번 agent loop 연결의 CLI 패키지 회귀 검사 172개·실제 worker Python 검사 10개·Clippy를 통과했다(live Rust 검사 3개 ignored).
- 실제 graph-tool-call 0.46.0과 CLI·loopback model endpoint로 두 명세의 목록 요청에서 검색 → schema 조회 → 최종 응답 연결을 검증했다. 실제 LLM·업무 API 호출은 없으며 품질 비교가 아니다. 승인 전 차단, 재개·입력 digest·snapshot 검증, 변경 snapshot의 실패 Receipt를 확인했다.
- 서로 다른 OpenAPI 3.1/Swagger 2.0 fixture의 상세 요청과 별도로 보류한 목록 요청을 실제 worker·CLI로 검증했다. 현재 Python 검사 10개와 기존 Rust transport/lifecycle 검사 7개, 새 discovery·manifest·profile·driver 검사를 유지한다. 모델 호출·업무 API 실행은 없다.
- 기존 실제 명세 snapshot도 CLI로 가져왔다. 도구 1,108개, 저장 6,860,637 bytes, 최초 import 6.79초·search 3.44초였다. 검색 첫 후보의 업무 혼동은 남아 있고, 성능 향상이나 실제 두 시스템 통합 성공을 주장하지 않는다.
- 공개 Swagger UI 주소에서 명세 15개·도구 1,108개를 CLI로 직접 수집하는 것도 통과했다(import 10.68초). 실제 업무 API는 호출하지 않았다.
- graph-tool-call main의 `f8bec76` / 0.46.0을 별도 체크아웃해서 확인했다.
- 의존 관계 데모와 공개된 7개 사례의 결정적 릴리즈 증거 재현 검사는 통과했다. 실제 LLM 업무 성공률 검증과는 다르다.
- 실제 한 시스템의 Swagger에서 명세 15개, 작업 2,221개를 수집하고 도구 1,108개의 그래프를 생성했다.
- 서로 다른 조회 요청 3개로 검색을 실행했다. 상품 상세에 주문상품 상세, 주문 상세에 정기주문 상세가 상위에 나오는 등 의미 선택이 부족했다.
- 임베딩 없는 대규모 검색의 의미·언어 간 검색 품질 저하 경고가 있었다. 임베딩 추가 효과는 아직 검증하지 않았다.
- 초기 별도 테스트는 명세 수집·그래프 생성·검색까지였다. 이후 CLI·agent loop의 fixture 통합 검증은 위와 같이 수행했으며 실제 업무 API 호출은 아직 없다.
- 두 번째 시스템은 화면의 API 게이트웨이 주소만 확인했다. OpenAPI 명세 위치는 미확인이다. 화면 수집 경로는 제안 단계다.
- 따라서 두 시스템에 걸친 통합 성공이나 검색 품질 개선을 주장하지 않는다.
- SEV 문서·프로토콜·UI 데이터/평가 코드·기존 graph-tool-call 평가 연결을 검토했다. 문서상 브라우저 실행기는 browser-use가 아니라 agent-browser다. 최신 음성 실험 브랜치와 모델 서버 상태는 이 환경에서 확인하지 못했다.
- SEV의 API 후보 선택 실험은 이미 있지만 브라우저·API·xgen 통합은 아직 없다. 화면 요소 refs는 현재 스냅샷에 묶고 영구 도구 ID로 저장하지 않는 설계를 제안했다.
- 상세 근거와 통합 과제는 [브라우저·도구 그래프·SEV 검토](docs/development/browser-tool-graph-sev-2026-10-06.md)에 기록했다. 해당 문서의 결과는 기존 기록을 검토한 것이며 새 실행 실험은 아니다.

- [HTTP 조회 실행 계약·사용법](docs/development/http-read-tools.md): `tools connect` 한 번 설정한 주소·로컬 auth 참조를 Run에 고정한다. GET scalar 입력·2xx JSON response subset이며 원본이 없는 기존/원격 collection은 실행 거절한다. 두 새 fixture의 실제 HTTP·승인/입력/연결 drift·실패 Receipt·kill 후 중복 방지를 검증했다. 실제 LLM·live 시스템·OS keyring backend 검증은 아니다.

- HTTP 조회 추가 후 CLI Rust 검사 178개 통과·3개 live 검사 ignored, protocol 13개·Python 도구 검사 29개(새 HTTP 검사 8개 포함)·Clippy·fmt·공개 문서 검사 통과. release에서도 두 새 API fixture의 검색→GET→최종 응답·완료 재생을 재확인하고 로컬 바이너리를 갱신했다.

- [검색 책임 분리 평가](docs/development/tool-search-evaluation-2026-10-06.md): Gitea/WireMock 원본 고정, 두 시스템 16 intent의 영어·한국어 요청 32개에서 목표 endpoint top-5 18/32. 직접→CLI 후보/명세·CLI→agent search 차이 0건, describe 14건은 64 KiB 초과·실패 Receipt로 확인했다. 원본에 없는 cluster-wide 설명도 양쪽에서 재현했다. 실제 LLM 단계는 전달 gate 실패로 미실행이며 기본값/엔진/업스트림 코드는 변경하지 않았다. 평가 실행기 검사 5개 통과, 기존 모델 평가 실행기 검사 11개 통과·built example 검사 1개 skipped, 공개 문서 검사를 통과했다.

- [제한된 모델용 describe](docs/development/bounded-tool-describe.md): describe만 2.0.0으로 변경하고, 48 KiB view·입력 페이지·생략 표시·전체 후보/HTTP 계약 digest를 제공한다. 별도 호스트 경로는 전체 원본 계약(최대 8 MiB)으로 입력·응답을 검증한다. CLI 기본 describe는 전체 후보를 유지한다. 큰 enum·중첩 schema의 세부 모델 조회는 아직 없고 검색 품질 개선은 별도다. 이전 진행 중 discovery Run은 새 Run이 필요하며, 완료 Run 재생은 확인했다. [별도 결과](docs/development/bounded-tool-describe-2026-10-06.md): 이전 32개 설계 회귀의 전달 실패 14→0, view 최대 6,641 bytes·64개 Receipt 검증 통과, 검색 top-5 18/32는 동일하다. 추가 검증 두 fixture에서 생략된 원본 입력/응답 제약·정상 재생을 확인했다. CLI 179개·protocol 13개·Python 도구 34개·평가 실행기 5개 통과. 실제 LLM 선택 품질은 아직 미평가다.

- [실제 모델 검색어·선택 평가](docs/development/live-tool-choice-2026-10-06.md): 새 8개 조회/2개 없는 작업 intent의 영어·한국어 요청 20개×2회, 실제 DeepSeek 120회. 동결 원점수는 정답 선택 16/32→26/32, 없는 작업 거절 두 조건 8/8. branch-tip의 유효한 git ref 대체 API를 정답에서 빠뜨린 기준 결함을 발견해 원점수를 유지하고 해당 4회는 결함 판정에서 제외했다. 나머지 7개 조회 intent 28회의 사후 분석은 선택 16→26, 후보 포함 조건부 선택 16/16·26/26. 전달 차이 0건·39개 Receipt 검증 통과, 비용 상한 계산 $0.063643296·미해결 예약 0. 실제 전체 agent 업무 완료나 엔진 개선의 전후 비교가 아니다. 검색/query 실패 소유자를 분리하고 production planner 재생과 검색 ablation을 다음으로 둔다.

## 다음 과제

작업 목록과 완료 기준의 정본은 [도구·모델 통합 TODO](docs/development/tool-integration-todo.md)다. 이 메모에 체크리스트를 중복 관리하지 않는다.

순서: 공통 계약·검증 계획 → 내부 도구 검색 → API 조회 실행·연결 UX → 브라우저·SEV → API·화면 그래프 통합 → 역할별 모델·STT → 배포·기본값 결정.

공통 계약·검색과 `xgen.http/read`의 GET 실행·연결 저장을 구현했다. 모델용 명세 조회와 호스트 실행 계약을 분리해 전달 gate를 통과했다. 실제 모델 query/선택 baseline은 완료했고, 다음 우선순위는 정답 기준 audit 후 production planner 재생과 graph-tool-call 검색 ablation이다. graph-tool-call 검색/정규화 개선은 해당 저장소의 별도 과제로 진행한다. 원격 원본 보존·연결 UX·HTTP 지원 확대와 브라우저·SEV 연결도 남았다. 모든 항목은 일반화 가능한 기능으로 개발하고, 설계 사례와 별도로 보류한 검증 사례를 사용한다.

## 도구별 개선과 비용 추적 방식

작업이 생길 때 아래 형식으로 이 파일에 항목을 추가·갱신한다. 별도 도구 저장소에 구현하면 커밋 또는 이슈를 연결하고, 자세한 증거는 해당 저장소의 기록을 참조한다.

| 필드 | 기록할 내용 |
| --- | --- |
| 소유 구성 요소 | xgen CLI / graph-tool-call / SEV / browser-use·agent-browser / 변환 어댑터 |
| 문제·근거 | 재현 입력, 관찰 결과, 실패 단계, 관련 증거 |
| 일반화 | 설계 사례와 보류한 검증 사례, 사례별 변경이면 적용 범위·제거 조건 |
| 상태 | 제안 / 구현 중 / 구현 완료 / 검증 완료 / 보류 |
| 결과 | 작업 성공률, 잘못된 도구 선택, 빠진 선행 도구, 실행·관찰 일치 여부 |
| 비용 | LLM 호출·토큰·금액, 브라우저 단계, 소요 시간, 그래프 생성·검색 비용 |
| 변경 추적 | 변경한 저장소·버전·커밋, xgen 호환성, 다음 과제 |

현재 도구별 개선 후보:

- graph-tool-call: 고정 검색어에서도 목표 endpoint 누락이 두 시스템에서 재현됐다. 범용 검색/색인·언어 처리와 원본에 없는 scope 설명 주입을 해당 저장소에서 개선·평가한다. xgen의 제한된 view와 별개로 업스트림의 색인 표현 계약도 검토 대상이다. ranking의 세부 원인과 개선 방식은 ablation 전이라 미확정이다.
- graph-tool-call 검색 1차 ablation(2026-10-07): 의미 채널을 융합에서 빼면 순위 정밀도와 지연은 좋아지지만, 그 채널의 action/shape 별칭(list/목록 등)이 영어 요청 x 한국어 명세의 거의 유일한 교차언어 신호여서 held-out recall이 그대로이거나 떨어졌다(불채택). 주 누락 원인은 교차언어다. 후보: 수집한 명세의 문서 언어를 xgen 검색어 생성에 전달(xgen 측, 사전 등록 필요), 선택형 임베딩. keyword.py의 하드코딩 ko->en 사전과 벤치마크 특화 의도 규칙은 기술 부채이며 확장하지 않는다. 이번 held-out은 소진했으므로 다음 판정에는 새 held-out이 필요하다.
- 검색어 언어 힌트(2026-10-07, 채택): 교차언어 누락은 planner 검색어 언어로 줄인다. 요청과 문서의 문자 체계가 다를 때만 힌트를 넣고, 예시 문장 없이 문자 체계 이름과 "요청 언어와 문서 언어 용어를 함께" 지시만 준다. 같은 언어 요청에 문장을 더하면 검색어가 흔들리고, 문서 예시를 보여주면 모델이 예시를 베끼며, 완전 번역은 영어 operationId 매칭을 잃는다. 세 번째 held-out 59 -> 73. 평가 기록: ~/.local/share/xgen/evaluations/language-hint-2026-10-08/PLAN.md (로컬, 사내 명세 포함). held-out 3개(10-08~10-10)는 모두 소진.
- production planner 재생(2026-10-07 진행 중): 실제 `xgen run`의 실행 DB(run.sqlite3 tool_outputs: search request.query·candidates, describe tool / usage.sqlite3 토큰)에서 검색 행동을 꺼내 오프라인 하네스로 재채점한다. held-out #4(10-11, 공개 명세 3개)와 사전 등록은 ~/.local/share/xgen/evaluations/planner-replay-2026-10-11/. 실행당 약 14초, 입력 약 17K 토큰. DeepSeek 잔액이 부족하면 402가 xgen에서 `provider_rejected`·`transport_failed`로만 보이니 대량 실행 전 /user/balance를 먼저 본다.
- graph-tool-call scope 설명 주입(2026-10-08, 채택, 901cd9d, 0.47.0 배포): 0.46.0 ingest가 K8s를 상정해 깊은 path의 description에 `(cluster-wide)` 등을 붙여 planner에 거짓 설명을 보였다(42%). 검색 효과는 전부 BM25 토큰 경로였으므로 색인 전용으로 옮기면 순위가 바이트 단위로 같다. 이런 '보이는 텍스트 vs 색인 텍스트' 분리는 순위 무변경 확인으로 판정이 쉽다. 평가: ~/.local/share/xgen/evaluations/engine-scope-2026-10-12/PLAN.md, held-out #5(engine-heldout-2026-10-12)는 소진. #4도 10-08 planner 재생으로 소진, 미소진 held-out 없음.
- production planner 재생 결과(2026-10-08): 단독 생성 검색어는 production 첫 검색어의 대리 지표로 쓸 만하다(오프라인 hit@5 차이 2~3/48). 언어 힌트는 production 유지 기준 1 불충족(61 -> 59)으로 재설계 대상. 핵심 함정: graph-tool-call `retrieve_with_scores`의 앞 순위가 top_k에 따라 바뀐다(top_k=5 vs 20의 앞 5개가 34/96 상이). planner는 topK 10·20을 쓰므로 엔진 평가는 같은 topK에서 앞 5개를 본다.
- top_k 의존 순위 수정(2026-10-09, 채택, graph-tool-call b9f7b8a, 0.47.0 배포): 원인은 `_preserve_dominant_keyword_candidates`가 top_k번째 점수를 기준선으로 쓴 것 하나. 고정 앞 5개로 바꾸자 held-out #6 모델 검색어 88 -> 91, 불일치 55 -> 0. 보정을 없애면 떨어지므로(설계 480 -> 453) 보정 자체는 유효했다. 진단법: top_k를 쓰는 단계를 하나씩 고정해 불일치가 사라지는 단계를 찾는다. held-out #6(engine-heldout-2026-10-13)은 소진, 미소진 없음.
- backend 버전 올리기(2026-10-09, 0.46.0 -> 0.47.0): collection envelope는 만든 버전에서만 열리므로 이전 버전 collection이 하나라도 남으면 `tools list`와 도구 검색 Run 전체가 실패한다. 오류를 `collection_backend_version_mismatch`로 분리했고 복구는 파일 삭제 후 같은 이름 재import(연결 설정은 이름 기준이라 유지). 고정값은 tools.rs 하나가 아니라 graph_discovery·composition·http_read·worker·capability 정의 fixture 3개 등 15곳이다. 과거 평가 문서·등록 fixture의 0.46.0은 기록이므로 바꾸지 않는다.
- browser-use / 변환 어댑터: 화면 수집 결과의 공통 계약과 갱신 조건 설계. 아직 구현·검증하지 않았다.
- SEV: 기존 API/UI 판단을 통합 호출 계약으로 연결하고, 없는 대상의 오실행·후보 품질·비용을 별도 평가. 같은 프로토콜만으로 API/UI 체크포인트를 통합하지 않는다.
- xgen CLI: 검색·describe와 연결된 GET 실행·인증 참조·Receipt·완료 재생을 구현했다. 두 독립 loopback API에서 검증했다. 큰 describe 전달 실패 14/32건은 모델용 view·입력 페이지·별도 실행 계약으로 수정하고 같은 설계 회귀에서 0/32건을 확인했다. 실제 LLM 검색어 생성/선택 baseline은 완료했다. 다음은 실제 production planner query 재생과 별도 검색 ablation이다. 큰 schema 세부 조회는 필요성과 선택 영향을 확인한 뒤 추가한다. 원격 명세 실행·연결 점검/수정 UX·실제 OS secret store backend·즉시 취소도 남았다.
- graph-tool-call 연계: 정규화된 api_contract만으로 원본 HTTP 스키마를 복원할 수 없었다. xgen worker가 로컬 JSON 원본을 추가 보존하도록 했으며 remote URL/Swagger UI 원본 보존은 후속이다. 업스트림 코드는 변경하지 않았다.
- 명세 없는 API 복원 PoC(2026-10-09, 프론트 번들 정적 분석): jsluice는 템플릿 URL(`/api/lists/${id}`)을 놓쳐 acorn AST 추출기를 따로 만들었다. 정밀도는 높지만(4앱 89~100%) 복원율은 프론트 호출 방식에 좌우된다(fetch wrapper 함수를 쓰는 Traccar 16/117). 압축 후에도 클래스 메서드 이름은 살아 있어 요약으로 쓰는 게 LLM 설명보다 낫고, 이름이 사라진 경우만 LLM이 돕는다. 공식 명세가 낡아 번들이 더 최신 API를 보여주는 경우가 있다(listmonk 23개). 검증셋(Kavita·Traccar)은 소진. 평가: ~/.local/share/xgen/evaluations/bundle-api-2026-10-09/PLAN.md
- 명세 없는 API 복원 PoC 2(2026-10-10, Spring jar 바이트코드): 복원은 거의 완전(recall 99.7~100%, 정답 밖 초과분 대부분 springdoc @Hidden 실제 라우트)인데 검색은 원래 명세에 짐. 원인은 한국어 질의 대 영어 식별자(한국어 12건 중 1~3건만 hit, 영어는 같거나 우세). 복원 품질과 검색 품질은 별개라 언어 맞춤 설명 단계가 필요. 큰 초안은 DTO inline 시 로컬 소스 5MB 상한 초과 -> components $ref. 평가: ~/.local/share/xgen/evaluations/jar-api-2026-10-10/PLAN.md
- 언어 병목 비교(2026-10-13): 검색어 힌트(Q)는 새 공개 held-out에서 hit@5 76 -> 80으로 유지했다(hit@1은 43 -> 37). 수집 시점 summary 보강(S)은 hit@5 87, hit@1 66으로 가장 좋았지만, QS 대 Q 판정에서 한 칸이 3 하락해 불채택했다. 보강 효과 대부분은 같은 언어 칸의 summary 보강에서 나왔고, 한국어 요청 x 영어 명세와 summary 없는 명세는 오르지 않았다. graph-tool-call 0.47 색인은 summary(없으면 METHOD /path)만 쓰고 description은 쓰지 않는다. 다음 후보는 S 단독 대 Q 직접 판정이다. 평가: ~/.local/share/xgen/evaluations/language-sides-2026-10-13/PLAN.md
- npm scope `@xgen-ai`와 bootstrap 게시(2026-10-24, 게시 완료·레지스트리 확인): ADR-0031이 정한 `@xgen` scope는 **다른 계정이 이미 선점**하고 있었다. 그 아래 게시된 package는 0개지만 npm은 user와 org가 같은 namespace라 org로도 못 만든다. 판별은 `registry.npmjs.org/-/org/<이름>/user`가 200이면 선점, 404 `Scope not found`면 빈 이름이다. 레포 runbook이 "scope 확보가 불명확하면 게시하지 말고 naming ADR을 먼저 갱신"으로 fail-closed를 걸어 둬서 추정으로 진행할 수 없었다. 실측 가용: 빈 이름 `@xgen-ai`·`@xgen2bee`·`@xgencli`, 선점 `@xgen`·`@x2bee`·`@plateer`·`@plateerlab`·`@xgenai`. `@xgen-ai`를 골랐다(회사 공개 배포 식별자가 이미 그 이름). 개인 계정 scope는 소유권 이전 시 게시된 6개를 deprecate해야 해서 뺐다. ADR-0051을 쓰고 ADR-0031·0034·0046 상태 줄이 그것을 가리키게 했다(본문은 당시 기록이라 `@xgen` 유지, 0031이 0034에 한 방식 그대로). rename 함정: `@xgen/` 문자열 치환이 **네 곳을 지나친다** — `npm/scripts/smoke.mjs`의 `path.join(globalRoot, '@xgen', 'cli')`, `npm/scripts/check.mjs`의 이스케이프된 정규식 `/^@xgen\/cli-.../`, 문서 두 곳의 slash 없는 scope 언급. 앞 둘은 동작 경로라 놓치면 smoke install과 contract check가 옛 이름을 찾는다. bootstrap 6개는 `0.0.0-bootstrap.0`으로 platform 5개 먼저·launcher 마지막 순서로 `--access public --tag bootstrap` 게시했고, 내용은 `package.json`·`README.md`·`LICENSE`뿐이며 `bin`도 `scripts`도 없다(설치 시 실행되는 코드 없음). 게시 직후 registry API는 몇 분간 404다 — "being processed"는 정상이고 전파를 기다려야 한다. GitHub은 secret `NPM_TOKEN` 갱신 + variable `XGEN_NPM_PUBLISH_ENABLED=true`까지 했다. repo variable에 `XGENY_*` 4개가 개명 잔재로 남아 있고 release workflow는 `XGEN_*`만 읽는다(미정리).
- `run`·`resume`의 진행 표시(2026-10-24, 구현 완료·로컬 검증): `--debug`는 전역 플래그이고 help에 "durable progress events를 보여준다"고 적혀 있지만 `run`·`resume`에서는 **파싱만 되고 버려졌다**(`cli.debug`의 유일한 사용처가 `interactive_command`). 조용히 아무 일도 안 하는 플래그였다. `run_local_with_progress`와 `resume_local_with_model_resolver_and_progress`는 이미 public이었고 `run`만 `run_local_with_started`를 고르고 있었으므로, 호출만 바꾸고 REPL의 `print_progress` 하나를 `pub(crate)`로 올려 같은 formatter를 공유했다. 설계 선택 둘. (1) spinner가 아니라 event 줄을 쓴다 — `run`의 stderr는 이미 `XGEN_*` 줄 계약이라 줄 단위가 pipe·로그와 맞고, spinner는 `XGEN_STARTED`와 커서가 겹쳐 `\r\x1b[2K` 조율이 필요하다(spinner는 프롬프트로 돌아갈 자리가 있는 REPL 것). (2) 기본값은 침묵 — 결과 줄만 파싱하는 host에 묻지 않은 `progress:` prefix를 주지 않는다. 쓰기 실패는 보고만 멈추고 Run은 계속한다(진단 한 줄 때문에 durable 작업을 버리지 않음, `run`의 중단은 process-level signal handler 소유). `resume`은 egress 유무로 갈렸던 두 경로를 `..._and_progress` 하나로 합쳤다(no-egress는 거절하는 resolver, 기존 `resume_local`과 동일). 검증: `--debug`에서 `progress: model_call_starting`·`response_committed`, 없으면 0건, stdout은 양쪽 모두 최종 응답 바이트만(테스트가 `assert_eq`로 고정). 플래그를 안 넘기도록 되돌려 테스트 실패 확인. 전체 691 passed·0 failed(4 ignored), clippy·fmt·공개 문서 검사 통과.
- `run`의 실행 파일 catalog(2026-10-24, 구현 완료·로컬 검증): `--allow-execute`만 주면 `configuration_mismatch`로 죽는데, 이건 버그가 아니라 **의도된 계약**이고 고정 테스트도 있다(protocol_check.rs). 이유는 매니페스트에서 확인했다. execution profile digest에 `environment:PATH`와 catalog된 실행 파일 각각의 해시가 들어가므로, PATH 자동 발견을 기본값으로 하면 같은 명령이 머신마다 다른 프로필을 만들고 `resume`이 프로필 불일치로 닫힌다. REPL은 한 프로세스·한 세션이라 자동 발견해도 되고 `run`은 아니다. 그래서 기본값을 뒤집지 않고 `--allow-path-executables`를 opt-in으로 더했다(`run`·`resume` 양쪽, resume에도 같은 플래그 필요). `--allow-executable` 항목은 그대로 유지되고 이름 충돌 시 이긴다(실측: 명시한 `git=/nonexistent/git`은 거절, 빼면 발견된 git으로 통과). 자동 발견은 **shell을 넣지 않는다** — ADR-0049가 "catalogued executable을 shell 없이" 시작하도록 정했고 PTY session은 Linux 전용이다. `sh -c "<명령>"`은 실제 명령을 argument에 숨겨 Receipt를 무의미하게 만든다. 따라서 Pocket Agent 쪽 shim의 `with_shell`(`--allow-executable sh=<path>`)은 이 플래그로 대체되지 않는다 — shell이 필요하면 host가 이름으로 직접 넣는 것이 설계상 맞는 경로다. 빈 catalog 실패는 어느 쪽이 빠졌는지와 두 가지 해결책을 함께 출력하게 했다(`PublicRunError::Configuration`은 54곳 공용이라 코드만으로는 아무 정보가 없다). 검증: loopback fake provider(probe=completion_candidate / 대화 턴=response_candidate 구분 필요, 안 하면 `completion_without_receipt_completed_plan`으로 보여 제품 결함처럼 읽힌다)로 `run`이 발견된 catalog + `--conversation-responses`로 exit 0 응답까지 확인. egress 동의 검사가 catalog 검사보다 **뒤**에 있어서, 동의 없이 돌리면 pause 지점이 catalog 수용 여부를 네트워크 없이 그대로 드러낸다 — 테스트에 이 성질을 썼다. 전체 690 passed·0 failed(4 ignored), clippy·fmt·공개 문서 검사 통과.
- provider preset과 base URL 규칙(2026-10-24, 구현 완료·로컬 검증): 터미널에서 opencode처럼 쓰려면 provider 선택이 먼저라 preset을 2개에서 5개로 늘렸다(deepseek·openai·openrouter·glm·local, 그 밖은 custom). 함정 셋. (1) `api_endpoints`가 base URL의 path가 `/v1`이길 **강제**해서 자기 API를 다르게 번호 붙이는 provider는 설정 자체가 불가였다(z.ai `/api/paas/v4`, Azure OpenAI `/openai/deployments/<name>`). `/v1`은 관례일 뿐이라 빈 path만 거절하도록 바꿨고 오류 코드는 `base_url_must_end_in_v1` -> `base_url_must_name_the_api_root`다. (2) preset은 사용자가 고르지 않은 값으로 실패할 수 있어 preset 자체를 검사해야 한다. `local`을 `http://localhost:11434/v1`로 썼더니 평문 HTTP는 loopback **literal**만 허용해서 네트워크에 닿기도 전에 `plaintext_endpoint_must_be_loopback`으로 죽었다. `127.0.0.1`로 고치고, 모든 preset이 설정 규칙(base path + transport security)을 통과하는지 보는 테스트를 넣었다(되돌려 실패 확인). (3) Claude는 preset에 넣지 않는다. OpenAI 호환 layer가 `response_format`을 무시한다고 문서에 적혀 있어서 `model setup`은 PASS하고 모든 Run이 깨진다. 설정은 성공하고 실행만 실패하는 preset은 편의가 아니라 지뢰다. OpenRouter 같은 gateway 경유는 된다. GLM은 pay-as-you-go와 Coding Plan의 경로가 달라(`/api/coding/paas/v4`) 주석·문서에 적었다. 검증: loopback fake provider로 picker 6항목·`local` preset·비-`/v1` 루트(`/api/paas/v4`)·trailing slash 정규화·bare host 거절 4종과 `model list|use|check`를 실측했다. preset의 `response_format` 값 자체는 DeepSeek·local만 실측이고 나머지는 provider 문서 기준으로 문서에 구분 표시했다. 로컬 전체 검사 688 passed·0 failed(4 ignored), clippy·fmt·공개 문서 검사 통과. 맥에서는 `scripts/tests` 중 `test_timeout_stops_owned_process_descendants` 하나가 `os.pidfd_open`(Linux 전용) 때문에 원래 실패한다 — eval 스크립트의 이식성 문제이고 제품 경로가 아니다.
- planner 입력 비용(2026-10-27, #124 ADR-0072, 구현·실측 완료): 긴 작업(H6·H7·T3, 360 call)에서 call당 평균 43K token 중 `toolOutputs`가 75%였고(그중 `read-text` 본문 77%, 나중에 바뀐 파일의 옛 본문 76%), cache 적중은 35%였다. 원인은 wire 순서다. 상태가 제자리에서 바뀌는 `steps`가 `toolOutputs` 앞에 있어서 그 뒤가 매번 cache 밖이었다. 순서만 `toolOutputs` → `steps`로 바꾸자 긴 작업 cache 적중이 85~91%, cache 밖 입력이 1/5~1/10로 줄었다. 내용·digest는 그대로이고 envelope profile만 v3이다. 이어서 읽은 자원 전부가 나중에 바뀐 관찰의 출력을 빼는 안(옛 `read-text` 본문 축약)을 실측했지만 채택하지 않았다. 입력 총량은 40~65% 줄었으나 cache 밖 입력은 그대로였고, 같은 파일 재독 수가 줄지 않았으며, 완료는 H6만 나아지고 H7은 3회 중 2회 → 1회로 줄었다. 비정상 종료도 8회 중 1회 → 4회였다(실패한 파일의 읽기 출력은 남아 있었으므로 직접 원인은 아님). 같은 날 재시도 temperature 0.7 실험은 반복 탈출률이 오르지 않아(H6 21% vs 기준 25%) 채택하지 않았다.
- 출력 상한 반복(2026-10-27, #127 ADR-0073, 구현 완료): 모델이 같은 edit를 수백 번 반복해 16,384 token 상한에 닿으면 Run이 `output_truncated`로 끝났다(응답 4,402개 중 2개). 같은 요청 재전송은 6/6 정상이라 잘린 응답도 ADR-0061처럼 연속 2번까지 재시도한다. 드문 현상이라 실행 A/B는 없고 재전송과 e2e 테스트로만 확인했다.
- 하단 고정 입력창(2026-10-28, #129 ADR-0074, 구현·tmux 실사용 확인): 터미널에서 입력창이 화면 아래에 고정되고 진행·결과는 위로 흘러간다(alternate screen 없음, scrollback 유지). REPL 코드는 그대로 두고 출력 바이트를 터미널처럼 해석하는 frontend thread를 두었다. Run 중 입력·대기열(다음 `xgen>`에서 실행)·Ctrl+C 취소·Alt+Enter 여러 줄을 지원하고, 기록은 `repl-history.jsonl`로 바뀌었다. rustyline을 crossterm 0.29.0으로 바꿨다. crossterm은 default-features를 끄면 `windows` feature도 빠져 Windows 빌드가 깨지므로 같이 켠다(브랜치 전체 matrix에서 발견). 창 축소 잔상은 ADR-0077에서 후속 수정했다. 남은 일: 긴 결과 접기.
- 지침 파일(2026-10-30, #131 ADR-0075, 구현·실측 완료): 대화형 세션이 설정 root의 `AGENTS.md`(사용자)와 workspace root의 `AGENTS.md`(없으면 `CLAUDE.md`)를 읽어 Run마다 goal 뒤에 붙인다(합쳐 32 KiB, goal 우선 고정 문장). planning context 필드·envelope은 그대로이고 goal과 함께 journal에 남는다. headless `xgen run`은 읽지 않는다. main 대비 각 2회: 설계 사례(테스트 스크립트 사용, 영어 응답) 0/2 -> 2/2, 보류 사례(사용자 파일 접두 규칙 10/10, 실제 12 KiB CLAUDE.md의 타입 힌트·logging 규칙 1/2 -> 2/2). 지침과 goal이 어긋난 사례(화면 출력 요청 vs stdout 금지)는 두 빌드 모두 goal을 따랐다. 남은 일: 긴 작업의 token 비용 측정, 하위 디렉터리 지침·@import.
- collection 운영 명령(2026-10-13, #88): `tools rebuild|disconnect|remove`와 `list` 상태를 추가했다. 함정 두 가지. graph-tool-call artifact에 빌드 시각(`built_at`, `created_at`)이 들어가 같은 입력도 다시 빌드하면 digest가 바뀐다. 그래서 rebuild는 backend 버전이 다를 때만 교체한다. 연결은 collection 이름으로 붙으므로, collection 파일만 지우면 같은 이름의 다른 API가 이전 주소와 토큰을 물려받는다. 그래서 remove는 연결부터 지우고, import는 연결만 남은 이름을 거절한다. CI의 Python 단계는 graph-tool-call 없이 돌아 worker·CLI 전 과정 테스트가 skip되므로 로컬에서 확인한다.
- filesystem 검색 ignore(2026-10-30, #133 ADR-0076, 구현·검증 완료): `search-text@2.0.0`이 workspace 내부 `.gitignore`·`.ignore`를 해석한다. 규칙 해석만 ripgrep의 `ignore` crate로 하고 기존 nofollow 탐색·workspace 경계·검색 예산은 유지한다. 규칙 읽기도 전체 byte 예산에 합산하며, 무시된 경로는 `truncated`의 원인이 아니다. Python·JavaScript 설계 layout 2개는 기존 128개 결과에서 소스를 놓쳤고 변경 후 소스 1개만 반환했다. 별도 Rust layout의 CLI E2E에서 glob·예외의 결과 2개, 무시된 파일의 명시적 읽기, 규칙 변경 뒤 완료 replay를 검증했다. 실제 LLM 완료율·token 비용은 측정하지 않았다. 전체 회귀 763개 통과(4 ignored), Clippy·fmt·protocol·문서·배포 workflow·라이선스 검사 통과. 코드 commit `04036d1`의 브랜치 전체 CI 7개 job(5개 플랫폼 release·설치 smoke 포함)도 통과했다. 소유자는 xgen filesystem adapter이며 graph-tool-call 변경은 없다.
- inline TUI resize(2026-10-30, #134 ADR-0077, 구현·검증 완료): pane을 실제 화면 하단의 절대 좌표에 두고, reflow 뒤 cursor·hard-line 폭으로 origin을 복구하며 줄 단위로 지운다. 기존 `CSI J`는 pane이 첫 줄로 올라오면 tmux에서 매 frame을 scrollback으로 보내 복사본을 증식시켰다. 작은 폭·높이와 대기열 표시도 화면 안으로 제한했다. 4,800개 layout 조합과 draft·queue·승인 보존 unit, 실제 tmux/loopback provider smoke를 통과했다(모델 요청 3개, 추가 재요청 없음). 같은 smoke는 기존 binary에서 40×12 축소 시 실패했다. 실제 LLM 호출은 없고 native terminal 화면 관찰은 Linux/tmux 범위다. 실행 중 resize로 socket 읽기가 끊기는 별도 문제는 `ureq` 3.5.0의 공식 EINTR 수정으로 해결했으며 자체 transport·HTTP 재전송을 넣지 않았다. 원본 socket 수정은 ureq 책임, 표시·입력과 의존성 선택·검증은 xgen 책임이다. 전체 회귀 766개(4 ignored)·Clippy·fmt·protocol·문서·배포 workflow·license 검사를 통과했다. 코드 `e751aef`의 전체 CI 7개 job(5개 플랫폼 release·설치 포함)과, CI 연결 보완 `02bd243`의 Linux PR 검사도 통과했다. 실제 tmux smoke를 Fast/Linux PR gate에 넣었다.

- 긴 결과 접기(2026-10-30, ADR-0078, 구현·로컬 검증 완료): 일반 TUI의 답변·Changes가 12행을 넘으면 앞4/뒤2행을 남기고 접는다. Ctrl+O 전체 보기, 좌우 result 선택, Home/End·페이지 이동, Esc 복귀를 제공한다. typed section 전달로 모델 문자열을 UI marker로 해석하지 않는다. 전체 보기에서 Enter·paste는 승인/goal에 전달하지 않고 background Run·새 승인·출력·입력을 보존한다. 최근20개/원문·행 배열8MiB 보관, 초과 section은 원문 출력, pipe/debug 기존 제한 유지다. Rust 773개(4 ignored), Python 130개(37 skipped), Clippy·fmt·protocol·공개 문서 검사와 실제 tmux 기본/결과 smoke(HTTP 3/5회)를 통과했다. 실제 유료 모델·다른 OS 터미널 실측은 하지 않았다. 빠른 연속 resize stress에서는 viewer를 열지 않은 draft·실행 중 queue에서도 이전 테두리 한 줄이 남았다. #134의 정상 resize 검증과 구분하며, clamp·margin·absolute-row 가설은 효과가 없어 버렸다. 변경 전 main `ad2a898`의 별도 archive 빌드에서도 40×12 → 100×30 stress 실패를 재현했다. `scripts/smoke-inline-tui.py --rapid-resize`로 후속 재현하며 이 문제 해결은 아직 아니다.
- 긴 결과 접기 변경 추적: [PR #135](https://github.com/PlateerLab/xgen-cli/pull/135), 코드 `45c4c0b`, [브랜치 전체 CI](https://github.com/PlateerLab/xgen-cli/actions/runs/41055760380). Linux PR 검사는 통과했고 전체 CI는 해당 run 결과를 참조한다.

- 빠른 TUI resize 잔상(2026-10-30, ADR-0079, 구현·로컬 검증 완료): tmux가 screen을 먼저 바꾸고 PTY 크기 통지를 늦추는 동안 xgen의 periodic redraw가 옛 크기로 다시 그렸다. cursor report 캐시 문제가 아니었다. Unix draw·정리는 cursor 위치를 보존하고 화면 밖 CUP→CPR clipping으로 실제 화면 크기를 조회하며, Windows console size API는 유지한다. sleep·tmux 감지·전체 화면/scrollback 삭제는 넣지 않았다. draft·실행 중 queue의 rapid smoke 5회와 별도 크기 12회 burst, 결과 viewer·승인·입력·history·종료 복원을 통과했다(모델 HTTP 3/5회, 유료 LLM 없음). Rust 773 passed·4 ignored와 Clippy·fmt·문서·배포 workflow 검사를 통과했다. PTY만 73×19·화면은 100×30으로 고정한 fixture에서도 새 입력 redraw를 확인해 지연 상황을 timing과 무관하게 검증했다. 이 강제 불일치 검사는 변경 전 main `6215e93`의 별도 archive 빌드에서 실패하고 수정 빌드에서 통과했다. Unix redraw마다 CPR 왕복 하나가 추가되어 원격 터미널 latency 비용은 남는다. 소유자는 xgen TUI이며 다른 도구 변경은 없다.
- 빠른 resize 변경 추적: [PR #136](https://github.com/PlateerLab/xgen-cli/pull/136), 코드 `f1171ba`, [전체 CI](https://github.com/PlateerLab/xgen-cli/actions/runs/41071470733). 강제 PTY 지연 fixture는 후속 PR 검사에 포함한다.

- 상태 폴더 권한 진단(2026-10-31, ADR-0080, 구현·로컬 검증 완료): Unix mode의 group/other 비트 거절은 유지하고, root·runs·개별 Run 폴더 오류를 `state_directory_permissions`/exit64로 전달한다. CLI run/resume/usage/recover·REPL에서 문제 경로·현재 mode·`chmod go-rwx -- PATH` 안내를 표시하며 자동 chmod·재귀 수정은 없다. 명시적 로컬 진단만 경로를 escape해 표시하고 일반 Debug·오류 Display·journal·모델·progress에서는 가린다. 설계 root0775/runs0755와 별도 run0770·symlink·space·개행/ESC·비UTF8 경로를 검증했다. 사용자가 직접0700으로 고치면 권한 검사 다음 단계로 진행하며 fixture DB는 생성되지 않는다. 전체 Rust 회귀778개(4 ignored)·Clippy·fmt·protocol·공개 문서·배포 workflow 검사를 통과했다. 소유자는 xgen CLI이며 외부 도구 변경은 없다.
- 상태 폴더 진단 변경 추적: [PR #137](https://github.com/PlateerLab/xgen-cli/pull/137), 코드 `7edf540`, [전체 CI](https://github.com/PlateerLab/xgen-cli/actions/runs/41077456833). 실제 외부 모델·다른 OS의 terminal 화면 실측은 하지 않았다.
- 상태 폴더 진단의 [Linux PR 검사](https://github.com/PlateerLab/xgen-cli/actions/runs/41077575969)가 통과했다. 플랫폼 전체 결과는 위 전체 CI run을 참조한다.

- HTTP 조회 확장 요청(2026-10-31, 사용자 승인): 범용 기능으로 선택 unsupported 파라미터 단위 제외 → query 배열(form/explode, Swagger multi) → operation별 호스트 지정 POST 조회 → Run별 bearer env 주입 순서다. 1번은 단독 PR로 먼저 제공한다. 승인 있는 쓰기는 대회 후 별도 설계다. 상세 정본은 [통합 TODO](docs/development/tool-integration-todo.md#http-조회-확장-요청-2026-10-31). 외부 커머스 API는 통합 사례로만 쓰고 설계 기준에 넣지 않는다. 생성기·endpoint·인증·비교기는 외부 통합 담당자의 책임이며 실제 실행 가능 과업 수는 미검증이다.
- 선택 HTTP 파라미터(ADR-0081, 구현·로컬 검증): override 병합 뒤 지원 밖 선택 항목만 제외하고 name/location/reason을 contract digest와 모델 describe에 포함한다. required/path·보안/응답 오류는 계속 operation을 거절한다. 기존 host의 closed parameter 검사로 제외 입력은 요청 전에 거절된다. assets/calendar 설계와 shipment/sensor 실제 loopback import→describe→GET·악의적 제외 입력 0회 요청을 확인했다. 기존 HTTP 실패 테스트의 즉시 종료 기대를 ADR-0052의 실패 관찰 후 보고 계약에 맞췄으며 엔진 실패 의미는 바꾸지 않았다. 자체 과제 평가는 [draft #138](https://github.com/PlateerLab/xgen-cli/pull/138)에 보존했고 모델 인증 대기는 계속된다.
- 선택 파라미터 검증 결과: Rust778개(4 ignored), system Python venv+graph-tool-call0.47.0+컴파일 CLI의 전체139개 검사(7 skipped), HTTP 관련18개 전부 통과했다. Clippy·fmt·protocol·문서·workflow 검사도 통과했다. UV standalone Python의 pidfd_open 부재는 기존 평가 process-cleanup 도구의 환경 제약으로 분리했고 system Python에서 전체 검증했다.

- HTTP 확장1은 [PR #139](https://github.com/PlateerLab/xgen-cli/pull/139)로 main `0c29e15`에 머지했다. [Fast/Linux PR 검사](https://github.com/PlateerLab/xgen-cli/actions/runs/41084874587) 통과.
- HTTP 확장2(ADR-0082, 구현·로컬 검증 완료): query 배열은 OpenAPI3 form/explode=true와 Swagger2 multi만 지원한다. scalar items와 배열 제약을 전체 schema로 검증하고 반복 query pair를 host에서 인코딩한다. 다른 직렬화·object items는 선택 제외/required 거절을 유지한다. 실행 profile은 한 상수 `xgen.http.read/v2`로 binding/profile에 고정한다. 이전 바이너리의 미완료 HTTP Run은 새 바이너리 resume에서 execution_profile drift로 요청0회 거절, 완료 Run은 네트워크 없이 재생됨을 실제 loopback fixture로 확인했다. 배열 wire·잘못된 값 0회 요청도 shipment/sensor에서 확인했으며 Rust781개(4 ignored)·pinned backend/컴파일 CLI 전체144개 검사(7 skipped)·Clippy·fmt·문서·workflow 검사를 통과했다.

- HTTP 확장2는 [PR #140](https://github.com/PlateerLab/xgen-cli/pull/140)로 main `4fbbcc9`에 머지했고 Fast/Linux 검사가 통과했다.
- HTTP 확장3(ADR-0083, 구현·로컬 검증 완료): host operationId 목록으로 GET/POST 조회를 지정하고 POST JSON 본문을 원본 schema로 검증한다. Capability2.0.0/profilev3로 고정하며 목록 변경은 기존 http_connections drift로 거절한다. shipment/measurement loopback POST·잘못된 본문0회 요청·미지정POST0회·drift를 확인했다. 실제 외부 서비스 통합과 모델 품질은 아직 미검증이다.
- HTTP 확장3 검증: Rust782개(4 ignored), Python148개(7 skipped), Clippy·fmt·protocol·문서·workflow 검사 통과. 전체 Rust 검사 도중 별도 빌드가 xgen 실행 파일을 바꿔 기존 executable drift 검사1건이 거절됐고, 빌드와 분리해 다시 돌린 전체 검사는 통과했다. 독립 diff 검토의 가설은 원본 계약·승인 경계·실행 전 body 검증과 대조했으며 추가 재현된 결함은 없었다.

- HTTP 확장3은 [PR #141](https://github.com/PlateerLab/xgen-cli/pull/141)로 머지했고 Fast/Linux PR 검사가 통과했다.
- HTTP 확장4(ADR-0084, 구현·로컬 검증 완료): run/resume의 연결별 bearer env override를 별도 map으로 받으며 저장된 연결 인증을 바꾸지 않는다. 참조만 manifest/profile에 결합하고 누락·변경·값 부재/invalid에서 fallback 없이 거절한다. shipment/sensor의 다른 토큰·연결 불변·상태/모델 비노출·토큰 회전·완료 replay와 echo 차단을 loopback에서 확인했다. 서버의 사용자 인증·대리 권한·Run 접근 제어·process/state 격리는 외부 호스트 책임이며 실제 통합은 미검증이다.
- HTTP 확장4 검증: Rust784개(4 ignored), pinned backend+컴파일 CLI Python153개(7 skipped), Clippy·fmt·protocol·문서·workflow 검사 통과. 한 Run의 연결2개/서로 다른 토큰도 올바르게 전달됐고 모든 상태 파일·모델 요청에 값이 없었다. 참조만 profile에 결합하며 override 없는 기존 binding은 유지했다. 승인 있는 쓰기는 아직 구현하지 않았다.
