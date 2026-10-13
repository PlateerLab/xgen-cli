# Planning boundary contract probe

- 기준일: 2026-10-05
- 상태: offline contract 평가 완료. 모델 pilot·외부 비교는 미실행.
- 변경 분류: 범용 평가. Production runtime·provider·protocol·journal schema 변경 없음.
- 사전 계획: [평가 프로토콜](../research/2026-10-05-planning-boundary-evaluation.md)
- 실행: [fixture·runner 안내](../../evals/planning-boundary/README.md)

## 무엇을 검증하나

실제 엔진의 admission·승인·intent·실행·typed output·검증·Receipt를 사용해. 별도 SQLite sink에서 attempt와 적용을 기록하고, parent가 child process를 강제 종료한 뒤 같은 Run을 다른 process로 재개해. Scripted planner는 plan과 Receipt-bound output만 보고, parent는 별도 oracle로 sink의 내용·cardinality·dependency 순서를 확인해.

한 Step씩 제안하는 조건과 최대 네 literal Step을 제안하는 조건의 planner 호출 수를 비교해. 외부 엔진을 돌린 결과나 실제 모델의 정확도·비용 비교가 아니야. Production planner policy도 변경하지 않아.

## 결과

동일 binary·Rust source·runner를 고정해 design 400회와 validation 400회를 순서대로 실행했어. 각 split은 2 fixture × packet size 1/4 × 10 scenario × 10회야. [전체 요약](../../evals/planning-boundary/results/2026-10-05/summary.json), [design 기록](../../evals/planning-boundary/results/2026-10-05/design/trials.jsonl), [validation 기록](../../evals/planning-boundary/results/2026-10-05/validation/trials.jsonl)에 개별 결과와 hash가 있어.

- Contract 통과 800/800, duplicate applied effect 0, false completion 0.
- 정상·plan commit·Receipt 직전 종료: 240/240 자동 완료.
- Model reservation 종료: 80/80이 먼저 recovery required로 멈췄고, 명시적 abandon 1회 후 완료했어. 이를 자동 복구로 세지 않아. Consumed reservation과 unknown count는 유지됐어.
- Sink 적용 뒤 outcome 저장 전 종료: 160/160이 manual 중단했고 자동 완료는 0이야. Query feature 광고 조건에서도 reconcile call은 0이었어.
- Adapter 실패 80회는 미완료로 남았고 descendants를 실행하지 않았어. 이미 계획된 독립 sibling은 처리했어.
- Catalog·material·profile 변경 240회는 effect 0으로 거부했어. Profile은 평가 host gate야.

| 정상 fixture | packet 1 planner calls | packet 4 planner calls |
|---|---:|---:|
| design literal | 3 | 2 |
| design observed chain | 3 | 3 |
| validation diamond | 5 | 2 |
| validation observed rounds·sibling | 5 | 4 |

각 값은 10회 반복에서 같았어. Frontier 복구와 반복 resume에서 추가 적용·planner 호출은 없었어. Literal batching은 scripted 호출 수를 줄였지만 관찰 경계를 없애지 않았어. Model token·비용은 NOT_MEASURED, 외부 비교는 NOT_RUN이므로 비용 채택 gate는 UNKNOWN이야. ADR-0050은 Proposed 상태로 유지해.

Trial latency는 process 시작·fault·resume·replay 검사까지 포함한 로컬 평가 시간이며 실제 모델 latency 비교가 아니야. Operator 작업 시간도 측정하지 않았어. 실패·중단 조건을 포함한 800회의 contract 통과를 작업 완료율 100%로 해석하면 안 돼.

첫 400회 batch에서는 다른 workspace build가 example binary를 덮어썼고, freeze 검사가 입력 변경을 검출해 해당 batch를 제외했어. Runner를 별도 binary copy로 고정하도록 보완한 뒤 design·validation을 다시 실행했어. 수정된 design 평가 중 원본 binary를 다시 빌드해도 동결된 copy는 영향을 받지 않았어. 결과의 binary SHA와 build provenance를 함께 기록했어.

검증: Rust workspace 654개, Python 50개 통과. 전체 Clippy·format·public docs contract 통과. GitHub Actions workflow는 추가하거나 변경하지 않았어.

## 복구 경계에서 발견한 제약

Planned admission이 `SinkGuarantee::None`을 고정하는 [구현](../../crates/xgen-runtime/src/admission.rs) 때문에, instance가 query feature를 광고해도 자동 query reconciliation을 수행하지 않아. Apply 뒤 outcome 저장 전에 종료된 non-idempotent 작업은 `manual_required`로 멈춰. 이를 자동 복구 성공으로 세지 않아.

추가로 [reconciliation의 Applied observation](../../crates/xgen-runtime/src/executor.rs)은 evidence digest만 반환하고 typed output은 반환하지 않아. 이 두 번째 제약은 소스 확인이며, 이번 admission probe에서 이 경로를 실행해서 검증한 것으로 주장하지 않아.

## 다음 작업

1. 범용: 동일 모델의 X0/X1/XN pilot fixture·wrapper·request profile·공통 budget을 고정하고 정상 완료 정확도와 token·비용을 비교해. 이번 scripted call 감소로 기본 policy를 바꾸지 않아.
2. 범용: 자동 복구를 확대하려면 host가 sink guarantee를 확인·binding하는 경계와 Receipt-bound typed output 복원을 별도 ADR로 정해. Feature 광고를 보장으로 간주하거나 unknown effect를 재실행하는 것으로 해결하지 않아.
3. 범용: typed dataflow·parallel scheduler·외부 엔진 비교는 각각 별도 gate를 유지해. 현재 결과로 아키텍처 우위를 주장하지 않아.
