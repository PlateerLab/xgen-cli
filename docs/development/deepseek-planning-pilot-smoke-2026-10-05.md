# DeepSeek smoke와 120회 pilot 사전 등록

2026-10-05. 범용 평가 작업. [실제 driver runner](planning-model-pilot-runner.md)의 live 연결·usage·budget을 확인하고 이후 비교 조건을 고정했어. 제품 CLI·engine·production prompt는 변경하지 않았어.

## 결과

| 단계 | 사례 / trial | 작업 oracle 통과 | 최종 claim 포함 합격 | 계산 비용 USD |
|---|---:|---:|---:|---:|
| 연결·계약 smoke | 2 / 6 | 6 / 6 | 6 / 6 | 0.006387558 |
| Design smoke | 4 / 12 | 12 / 12 | 11 / 12 | 0.015440088 |
| 합계 | 개발용 18 trial | 18 / 18 | 17 / 18 | 0.021827646 |

71 inference attempts에 HTTP 200·정확한 응답 model·usage/cache가 관측됐어. Unknown cost request, 중복 effect 시작, oracle 기준 false completion은 0이야. Price/token ceiling 위반·backend identity 변경·source/binary hash 변경도 없었어. 키와 raw provider body는 공개 결과에 넣지 않았고 private journal·workspace·driver logs는 `/tmp/xgen-deepseek-smoke-w4nub7ip`에 보존했어.

응답 model은 `deepseek-flash`, fingerprint는 `aeb56401ca74e127821c4f9126dcb669`로 같았어. 실제 underlying 모델 버전을 API 응답만으로 별도 증명한 것은 아니야. 공식 pricing page는 alias를 DeepSeek-V4.1-Flash로 안내해. [DeepSeek pricing](https://api-docs.deepseek.com/quick_start/pricing/)

2026-10-05 UTC 10:00–24:00은 공식 weekday peak 구간 밖이므로 이 window의 off-peak quote를 고정했어. 1M tokens당 cached input $0.003, cache miss input $0.15, output $0.60을 usage에 적용했어. 중국 공휴일 여부를 가정할 필요가 없는 시간대야. 비용은 공개 단가에 따른 계산이며 invoice 대조는 하지 않았어. [공식 시간 구간·단가](https://api-docs.deepseek.com/quick_start/pricing/)

공식 context length의 1M을 보수적으로 1,048,576 input tokens ceiling으로 두고 output은 4,096으로 제한했어. 매 요청의 cache miss 최대 reservation은 $0.159744야. `max_tokens`와 표준/native cache usage 계약도 확인했어. Reasoning tokens는 output의 subset이고 별도 과금하지 않아. [Chat Completions 계약](https://api-docs.deepseek.com/api/create-chat-completion/)

## 발견한 실패

Design의 `design-adjacent-runs` XN은 파일 수정과 hidden oracle를 통과했지만 최종 JSON의 명령 순서가 틀렸어.

- 실제 Receipt 순서: 실패한 `cargo test --offline` → 성공한 `cargo build --offline` → 성공한 `cargo test --offline`.
- 최종 claim 순서: 실패한 `cargo test --offline` → 성공한 `cargo test --offline` → 성공한 `cargo build --offline`.

수정 이후 test와 build는 같은 proposal의 독립 Step이었어. Candidate가 실제 실행 chronology 대신 예상한 순서를 답했다는 해석은 가설이야. Context에는 durable output chronology가 전달됐고, 이 한 사례만으로 구조적 원인을 확정하지 않아. 실패와 그 비용을 그대로 보존했고 fixture·prompt·engine을 이 사례에 맞춰 고치거나 성공할 때까지 재시도하지 않았어.

두 Rust 사례에서 X1의 model calls는 각각 6, XN은 각각 3이었어. Literal artifact는 4→2, 관찰로 다음 입력을 정하는 artifact는 4→4야. 이 수치는 개발용 1회 결과이며 성능 개선·일반화·채택 gate 통과를 뜻하지 않아. Cache 실비와 call count도 다른 지표야.

## 고정한 조건

- [Design 4개](../../evals/planning-model/design.json): literal artifact·관찰 뒤 입력 결정 2개, Rust project 진단·수정 2개.
- [Validation 4개](../../evals/planning-model/validation.json): TSV/관찰 기반 manifest artifact 2개, numeric/NDJSON Python CLI 진단·수정 2개. 모델 실행은 0회야.
- [Pilot config](../../evals/planning-model/pilot-config.json): X0/X1/XN 공통 `json_object`, thinking disabled, output 4096, request timeout 60초, model turns 24, ticks 512, trial timeout 300초. Core의 파생 model calls 48, planned/tool steps 24, context 512 KiB도 공통이야.
- 8 instances × 3 conditions × 5 repeats = 120 trials. 4 validation instance의 반복 20회를 독립 task 20개로 해석하지 않아.
- 전체 pilot cap $2.00. 개발 smoke 비용은 분리해. 충분한 평균 호출 여유를 확인했지만 worst-case 120회 전부를 cap 아래 끝낸다는 보장은 아니야. Guard가 schedule을 중단하면 미완료 결과로 기록해.
- Seed 202610052, balanced paired condition order, 직렬 실행, fresh Run/workspace, 자동 retry 0.
- Model ID·fingerprint, binary, config/fixture, engine/runner source, native cargo/rustc/python3와 최소 PATH를 hash로 고정했어. Rustup shim이 scratch HOME에서 실패하는 것을 피하려고 검증한 native Rust toolchain을 등록했어.

초기 입력이 oracle를 실패하고 올바른 reference는 통과하는지 점검했어. 이는 fixture/oracle의 개발 검사이며 validation 모델 trial이 아니야. Expected outcomes와 hidden test는 agent workspace·prompt·tool outputs에 전달하지 않아. Process 도구는 OS sandbox가 아니라는 기존 제한은 유지돼.

## 실행과 중단

[사전 등록 manifest](../../evals/planning-model/preregistration-2026-10-05.json)는 `REGISTERED_NOT_RUN` 상태와 정확한 schedule을 담아. 공개 snapshot은 [연결 smoke](../../evals/planning-model/results/2026-10-05/connectivity-smoke/summary.json), [design smoke](../../evals/planning-model/results/2026-10-05/design-smoke/summary.json), [공식 source hashes](../../evals/planning-model/results/2026-10-05/sources.json)에 있어.

Live smoke 당시 [runner snapshot](../../evals/planning-model/results/2026-10-05/runner-at-execution.py)도 보존했고 두 batch manifest의 source hash와 정확히 일치하는지 확인했어. 후속 preregistration/identity guard 추가와 smoke에 사용한 code version을 섞지 않아.

```bash
python3 scripts/evaluate-planning-model.py \
  --preregistration evals/planning-model/preregistration-2026-10-05.json \
  --binary /tmp/xgen-deepseek-smoke-w4nub7ip/planning_model_pilot \
  --output /absolute/path/to/new-private-pilot-results
```

Credential은 로컬 `XGEN_PILOT_API_KEY` 환경에서 읽어. 사전 등록을 사용하면 config·fixtures·upstream을 덮어쓸 수 없어. Source/config/fixture/binary/tool hash 또는 schedule이 달라지면 모델 I/O 전에 실패해. Model/fingerprint가 달라지거나, 고정한 identity를 확인할 수 없는 응답이면 해당 요청의 비용/unknown reservation을 기록하고 batch를 중단해.

현재 quote는 2026-10-06 UTC 00:00에 만료돼. 이후 실행하려면 공식 가격·backend를 다시 확인하고 **validation 요청 전에** 공통 quote와 사전 등록을 새로 고정해야 해. 실행 중에 가격·budget·prompt를 바꾸거나 실패를 성공으로 교체하지 않아.

채택 gate는 최소 두 validation family에서 XN 완료가 X1보다 적지 않고, 새로운 중복 effect 시작·false claim·false completion이 없으며, validation median 모델 비용이 15% 이상 낮아지는 것이야. Missing usage/cost는 UNKNOWN gate야. 전부 미완료라 낮은 비용이 나온 조건은 채택하지 않아. 4개 task의 descriptive pilot이며 통계적 superiority나 non-inferiority를 주장하지 않아. Gate를 통과해도 더 큰 확인 실험으로 이어가.

미승인 실행 0이라는 기존 gate도 유지해. 채택 판단 전에 validation journal의 authorization·invocation·material binding audit가 필요하며 단순히 trial을 완료했다는 사실로 대체하지 않아.

검증: Python 전체 61 passed. 실제 driver를 쓰는 로컬 success/rejection/wrong-claim 18회 계약 검사 포함. Model/fingerprint drift·missing identity, preregistration input/source/tool/binary/schedule 변경, ambient PATH와 등록 PATH의 분리도 검사했어. Rust source는 이전 workspace 659 passed 기준과 같고 이번 작업에서 변경하지 않았어. 최종 120회·외부 엔진 비교는 NOT_RUN이야.
