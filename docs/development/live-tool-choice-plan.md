# 실제 모델 검색어·도구 선택 분리 평가 계획

일반 평가 기능이다. xgen 엔진·검색기·모델 기본값은 변경하지 않는다. Gitea/WireMock의 이전 16 intent는 설계 자료이고, 이번에는 다른 8 intent와 시스템에 없는 요청 2개를 영어·한국어로 고정한다. 명세를 읽어 정답·허용 대체 도구를 정의했으며, 새 요청의 검색 결과나 모델 답변은 아직 보지 않았다.

## 고정 조건

- 같은 공개 commit 명세·graph-tool-call 0.46.0, 저장 artifact·binary hash, top-k 5, 각 요청 2회. 20개 요청·40개 trial이다. 순서는 고정 seed 20261006으로 섞는다.
- trial마다 실제 DeepSeek `deepseek-flash`, json_object, thinking disabled, temperature 0으로 검색어 생성 1회·원래 요청 후보 선택 1회·생성 검색어 후보 선택 1회: 최대 120회. 자동 재시도·중단 후 재호출은 없다.
- 원래 요청 후보와 생성 검색어 후보에서 같은 모델용 describe 2.0.0 view를 그대로 제공한다. 추가적인 요약/잘림 없이 최대 5개를 전달한다. digest와 원본 snapshot을 고정하고, 모델에게 label·대체 도구 판정·split·결과를 전달하지 않는다.
- 후보밖 선택/형식 오류는 잘못된 선택이다. 정답 후보가 없으면 모델의 잘못된 선택과 검색 누락을 따로 센다. 없는 작업은 null이 정답이며 관련 없는 도구 선택은 거절 실패다.
- 주 정답과 허용 대체 도구 기준을 각각 집계한다. 목록/검색 대체는 추가 exact filtering을 전제한 데이터 공급 능력만 인정한다. 한 번의 실행 성공이라고 주장하지 않는다.
- 실제 LLM 전 gate: 직접→CLI 검색 결과 동일, CLI model-view의 tool/snapshot/digest 확인, 새 요청의 CLI→agent search·describe 동일. 고정/생성 후보 입력은 전체 view이고 payload hash를 기록한다. 생성 검색어의 직접→CLI 차이가 나오면 이후 유료 호출을 중단한다.
- 요청 JSON 최대 60 KiB, 입력 token 한도 65,536, 출력 256, HTTP timeout 60초, 총 모델 timeout 900초, 응답 1 MiB. 반환 usage·모델/fingerprint를 검증하고 불명확한 비용은 예약 상한을 유지한 채 중단한다. 실제 요청 전 durable ledger 예약, 네트워크 redirect/proxy/retry 금지.
- 총 예산 상한 **$0.20**. 공식 가격표의 peak cache-hit $0.006/M, cache-miss $0.30/M, output $1.20/M을 상한 단가로 동결한다. off-peak 할인은 적용하지 않으며 청구액으로 단정하지 않는다. 가격·quote 유효시간·source hash는 실행 등록에 저장한다.

공식 단가·model alias 확인: [DeepSeek 가격표](https://api-docs.deepseek.com/quick_start/pricing/). 키는 기존 로컬 환경 참조로 읽고 모델 입력·결과·저장소에 넣지 않는다. GET model catalog로 광고된 이름을 확인했으며 추론 호출은 사전등록 후 수행한다.

## 판정과 채택

목표 endpoint의 primary/accepted top-5, 후보 포함 조건부 선택 정확도, 전체 선택 정확도, 후보 밖/형식 오류, null 거절 정확도, 2회 선택·검색어/후보 안정성, 호출·usage·시간·비용 상한을 집계한다. 두 언어 요청은 같은 intent의 짝이고 40개 trial을 40개 독립 업무라고 부르지 않는다.

- 직접 검색 누락: graph-tool-call의 검색·색인 과제. 원본 명세/정규화 결함은 별도 소유자다.
- 원래 후보에는 정답이 있고 생성 후보에서는 빠짐: 모델의 query 생성/검색 요청 구성 과제다. xgen production의 원인이라고 확정하지 않는다. 이번 query 단계는 standalone evaluator다.
- 정답 후보가 있고 전체 view도 전달됐는데 틀림: 모델/프롬프트 또는 view의 의미 정보 부족 가설이다. 원본과 view ablation 전에는 둘 중 하나로 확정하지 않는다.
- 직접→CLI→agent 차이: xgen 전달·컨텍스트 과제.

이번 작업은 baseline 측정과 원인 분리다. 채택할 변경은 없다. 검색/선택 방식 변경은 두 독립 시스템에서 재현하고 새 held-out 요청에서도 검증한 뒤 판단한다. 업무 API·xgen 실제 전체 agent의 업무 성공률·답변 정확성은 이번 측정 범위 밖이다.

사전등록 prepare a에서 유료 호출 전 snapshot pin 검토를 보완했다. 직접 검색용 graph 파일 hash와 실행 collection의 전체 artifact digest를 추가로 검증한다. a는 미완료 준비 실행이며 채택 근거가 아니다. 정답·요청·검색·모델 prompt·채점 방식은 변경하지 않고 b로 재등록한다.

유료 호출 전 의미 매칭과 실행 가능 여부를 구분하도록 선택 prompt를 명시했다. 모든 발견 view의 execution_enabled=false와 HTTP subset 지원 여부를 정답 매칭 근거로 사용하지 않는다. prompt 설명을 동결한 c를 최종 조건으로 사용하며 b도 미완료 준비 실행이다. 업무 완료나 현재 HTTP adapter 실행 성공률은 측정하지 않는다.
