# 도구 검색 책임 분리 평가 계획

2026-10-06. 아래 요청·조건·기준은 검색 결과를 확인하기 전에 고정한다. 이번 작업은 일반 평가 기능이며 엔진·검색 알고리즘을 수정하지 않는다.

## 입력과 범위

- 이전 assets/calendar/shipment/sensor fixture와 다른 실제 공개 명세: Gitea v1.24.6의 Swagger 2.0(452 operations), WireMock 3.13.1의 OpenAPI 3.0(41 operations).
- 원본 Git commit·raw bytes SHA256는 `scripts/fixtures/tool-search-evaluation/sources.json`에 고정한다. 전체 명세를 그대로 색인하고, 정답 도구만 추려 색인하지 않는다.
- 시스템마다 조회 intent 8개를 선정하고 영어·한국어로 각각 표현한 요청 16개, 총 32개를 `cases.json`에 고정한다. 영어 명세의 한국어 검색도 평가한다. 32개의 독립 업무가 아니라 16개 intent의 두 표현이다.
- 두 시스템은 엔진 설계용 fixture와 분리했지만 이 평가에서 읽은 요청/결과는 이후 검색 개선의 설계 자료로 취급한다. 개선 채택에는 추가로 보류한 시스템·요청이 필요하다. 이번 작은 진단 결과를 실사용 전체 품질로 일반화하지 않는다.
- 정답은 원본 method/path/operationId에서 결정한다. 평가 label을 색인·검색·모델 입력에 넣지 않는다. 명세의 설명을 자동으로 한국어 번역하거나 평가에 맞게 보완하지 않는다.
- graph-tool-call 0.46.0의 현재 기본 검색 설정을 사용한다. 임베딩·추가 reranker 없음. 검색 top-k=5, 동일 요청의 직접/CLI 비교 3회. 한국어 성능 저하를 지원 언어 계약 위반이라고 단정하지 않는다.

## 단계와 판정

1. 공개 API로 원본을 직접 graph artifact로 변환하고 CLI import 결과와 전체 도구 정의를 비교한다. 조회 정답의 존재·method/path·원본 설명·필수 parameter를 확인한다. 발견 계약의 정규화와 완전한 HTTP 실행 스키마 보존을 구분한다.
2. **같은 저장 artifact**를 graph-tool-call 공개 `ToolGraph.load`/`retrieve_with_scores`로 직접 호출한 결과와 `xgen tools search/describe` 결과를 비교한다. 직접 경로에서 xgen worker의 검색 함수를 재사용하지 않는다. tool ID·순서·description·score·producer 후보·정확한 tool 계약이 같아야 한다.
3. 고정 proposal을 쓰는 loopback endpoint로 agent loop를 실행한다. search 후 관찰한 첫 후보를 describe하고 다음 모델 요청에 실제 전달된 output을 검사한다. gold 도구로 강제 선택하지 않는다. 이 단계는 전달 검사이며 LLM 선택 정확도가 아니다.
4. 전달 검사를 통과하면 동일 공개 요청으로 실제 LLM의 검색어 생성과 후보 선택을 별도 통제 실험한다. 원래 검색어 후보에서 선택하는 조건과 모델이 생성한 검색어 후보에서 선택하는 조건을 비교한다. 모델 입력은 요청과 실제 후보의 ID·설명·method/path·입력명/타입/필수여부다. 이 선택용 projection은 두 조건에 동일하게 적용하며 전체 응답 스키마 전달 시험과 구분한다. 전체 xgen agent loop의 실사용 성공률과 구분한다.

검색 지표: top-1, top-5, MRR@5, 영어/한국어·시스템별 결과. 전송 지표: 후보·순서·설명·score·producer·describe 계약의 exact equality, 정규화 결과 차이, agent input 차이, 반복 안정성. 비용: direct 검색-only와 artifact load 포함 시간, CLI end-to-end 시간은 따로 보고한다. CLI 시간에서 프로세스 시작·uv·JSON/그래프 로드 비용을 검색 알고리즘 비용으로 해석하지 않는다.

책임 판정은 결과 확인 전에 고정한다: 같은 입력의 직접 검색에서도 정답이 없으면 검색/색인/입력 정보 과제, 직접→CLI 또는 CLI→agent 차이는 xgen 전달 과제, 정답이 제공된 후보에 있는데 모델이 잘못 선택하면 해당 모델/프롬프트 과제다. 원본 설명·operation identity를 먼저 확인하며 단일 종합 정확도로 소유자를 결정하지 않는다. 전달 합격은 누락·변형 0건이며 검색 합격 기준이나 개선 효과는 이번 진단으로 주장하지 않는다.

## 실행 예산

검색·전달 검사는 유료 모델 0회·업무 API 0회다. loopback endpoint는 네트워크 전송·Receipt를 실제로 확인하지만 답변 품질은 측정하지 않는다. 각 intent 표현의 모델 실험은 검색어 생성 1회 + 고정 후보 선택 1회 + 생성 검색어 후보 선택 1회, 최대 96회다. 1회 출력 512 tokens·사용량 입력 상한 8192 tokens·직렬화 request 7168 bytes 사전 제한·timeout 60초, 재시도 없음, 전체 예약 예산 $0.20 상한. 같은 prompt·후보 순서를 유지하고 temperature=0을 사용하며 1회 시행의 표본 한계를 기록한다.

모델은 기존 DeepSeek flash 설정으로 `json_object`, `thinking disabled`를 사용한다. [공식 가격표](https://api-docs.deepseek.com/quick_start/pricing/)에서 확인한 peak/cache-miss 입력 $0.30/1M·출력 $1.20/1M을 비용 상한으로 사용한다. 캐시·off-peak 할인은 적용하지 않으며 청구 영수증이 아닌 보수적 추정이다. quote 유효기간은 실행 전 manifest에 고정하고, 사용량이 없거나 예산/identity가 불명확하면 후속 모델 호출을 중단한다. 인증은 로컬 환경에서만 읽고 그래프·보고서·delegate에 전달하지 않는다.

결과 파일에는 계획·cases·sources·binary·평가 script의 digest와 실제 backend/model identity를 연결한다. 모델 실험은 명시적 `--live`와 별도의 동결 manifest가 있을 때만 수행한다. 입력이나 실행 파일이 바뀌면 기존 결과를 덮어쓰지 않고 새 평가로 분리한다.

가격 유효기간이 지난 뒤 실제 모델을 재실행하려면 새 quote와 새 manifest를 만들고 새 결과로 분리한다. 기존 quote를 수정해 이전 평가를 재사용하지 않는다. request bytes 제한은 tokenizer 계산값이 아니며 실제 usage가 입력 상한을 넘으면 후속 호출을 중단한다.
