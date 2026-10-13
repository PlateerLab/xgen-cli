# 모델 없는 planning boundary 평가

변경 분류: 범용 engine contract 평가. Production runtime·prompt·provider는 변경하지 않아. Scripted planner의 결과로 실제 모델의 정확도·비용 개선을 주장하지 않아.

```bash
cargo build --locked -p xgen-cli --example planning_boundary_probe
python3 scripts/evaluate-planning-boundary.py \
  --binary target/debug/examples/planning_boundary_probe \
  --output /tmp/xgen-planning-design --split design --repeats 10
python3 scripts/evaluate-planning-boundary.py \
  --binary target/debug/examples/planning_boundary_probe \
  --output /tmp/xgen-planning-validation --split validation --repeats 10
```

Output 경로는 새 디렉토리여야 해. Design에서 tooling을 수정했다면 hash를 다시 고정하고 validation을 실행해. 추가 GitHub Actions job이나 외부 API 호출은 없어.

## 실행 계약

실제 `AgentLoop`·`RunDriver`·admission·executor·verifier·SQLite journal을 사용해. Run과 독립된 SQLite sink는 execution attempt와 applied effect를 각각 append하고 중복 제거를 하지 않아. Python parent가 지정한 boundary의 JSON handshake를 확인한 뒤 child process를 강제 종료하고, 새 process로 같은 Run을 재개해. Sleep이나 log substring을 fault trigger로 쓰지 않아.

Planner는 fixture의 `plan`만 받아. 후속 operation은 Core Receipt로 검증된 `PlanningContext.tool_outputs`에서 발견해. Oracle는 parent가 별도로 읽는 `oracle`에 있어. 이 fixture들은 실제 Rust/Python project 작업을 대신하는 model-free contract 사례야. Validation은 별도 DAG·관찰 구조를 쓰지만, 모델에 대한 blind held-out 일반화 평가가 아니야.

| 조건 | 예상 결과 |
|---|---|
| normal | Oracle와 정확히 일치하는 적용·Receipt, 고정된 planner 호출 수 |
| adapter_failed | 실패 후 완료 보고 없음, descendant 차단, 이미 계획된 독립 sibling 처리 |
| plan_committed | 저장된 frontier 재개, planner를 중복 호출하지 않음 |
| before_receipt | effect 재실행 없이 verification을 재개 |
| sink_no_query / sink_advertised_query | 중복 없이 manual 중단. 자동 완료로 세지 않음 |
| model_reserved | 먼저 recovery required, 명시적 abandon 후 재개, 소비된 reservation 유지 |
| catalog_drift / material_drift | 승인 뒤 계약·입력 변경 시 effect 0 |
| profile_drift | 평가 host manifest 불일치 시 effect 0 |

현재 planned admission은 `SinkGuarantee::None`을 고정해. Instance의 query feature 광고는 sink guarantee가 아니므로 `sink_advertised_query`에서도 query가 호출되지 않아. 이 경로의 자동 query 복구는 `NOT_IMPLEMENTED`야. `AdapterReconciliationObservation::Applied`가 typed output을 반환하지 않는 추가 제약은 소스에서 확인한 사실이고, 이 runner에서 해당 복구 경로를 실행한 결과는 아니야.

Profile drift는 평가 host의 manifest gate를 검사해. Product CLI의 모든 profile 변경·복구 경로를 검증했다는 뜻은 아니야. 이 파일 sink로 network timeout, 실제 remote effect, 서버 idempotency를 검증한 것으로 해석하면 안 돼. Sink는 평가를 위해 만들었고 production adapter가 아니야.

## 결과

Runner는 먼저 binary를 별도 임시 경로에 복사해서 build 중 덮어쓰기와 분리해. `manifest.json`은 실행 전 binary·fixture·Rust source·runner hash와 config를 고정해. `trials.jsonl`은 trial마다 완료·oracle·안전·contract 판정을 구분해서 남겨. Exception·timeout도 실패 trial로 남기며 성공 trial로 교체하지 않아. `summary.json`의 passed는 contract 통과 수이지 작업 완료 수가 아니야.

Model reservation 이후 fault는 scripted planner 진입에서 발생해. 실제 provider가 요청을 받았는지·추론했는지 검증한 결과가 아니야. Model token·비용은 `NOT_MEASURED`, 외부 엔진 비교는 `NOT_RUN`이야.

사전 계획과 실제 결과 해석은 [평가 프로토콜](../../docs/research/2026-10-05-planning-boundary-evaluation.md)을 참고해.

2026-10-05 실행 결과: [구현·해석](../../docs/development/planning-boundary-probe-2026-10-05.md), [전체 요약](results/2026-10-05/summary.json).
