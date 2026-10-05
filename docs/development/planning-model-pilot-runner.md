# 실제 driver 기반 계획 크기 pilot runner

2026-10-05. 범용 구현. [평가 profile](evaluation-planning-profile.md)의 다음 단위야. 모델 응답을 고정한 계약 검증을 제공하며, 실제 모델의 성능 개선 결과는 아니야.

## 실행 구조

`planning_model_pilot` example은 제품 CLI와 같은 local composition·RunDriver·SQLite journal·filesystem/process adapters·승인·Receipt 경로를 실행해. 추가한 `run_local_with_evaluation_profile` API는 manifest 생성 전에 평가 Step 상한을 request profile에 결합해. 기존 CLI 진입점은 이 선택에 `None`을 전달해.

Python runner는 trial별 새 workspace·state·scratch·Run을 만들고 X0/X1/XN을 직렬로 실행해. X0는 기존 production reference, X1/XN은 공통 prompt의 상한 1/4야. Case/repeat마다 세 조건을 묶고 seed로 순서를 고정해. 여섯 순열을 반복해 순서 편향을 줄이지만 작은 task 수의 반복을 독립 task 증가로 해석하지 않아.

Agent의 모델 endpoint는 평가 전용 loopback proxy야. 실제 credential은 proxy의 `XGEN_PILOT_API_KEY` 환경변수에만 있고 bridge·도구·oracle 환경으로 전달하지 않아. Upstream은 HTTPS이며 credential 없는 literal loopback은 계약 테스트에서 허용해. Redirect·자동 retry는 하지 않아. Trial ID를 endpoint 경로에 넣어서 늦게 도착한 요청을 다른 trial 비용으로 분류하지 않아.

이 runner는 Linux `/proc`·pidfd를 요구해. Bridge/oracle timeout이면 생성한 process tree를 먼저 정지하고 pidfd로 종료해. Timeout trial 이후에는 batch를 중단하고 이미 기록한 실패를 새 성공으로 교체하지 않아. 일반 xgen CLI의 플랫폼 지원을 바꾸지 않아.

## 비용 계약

Config에 명시적 token ceiling, output bound, model-call/tick budget, request/trial timeout, 전체 spend cap과 quote를 고정해. 단가는 1M tokens당 정수 micro USD, ledger는 정수 nano USD야. 계산은 input cache hit/miss와 output을 합산하고 nano USD에서 올림해. Reasoning은 output의 subset이며 다시 과금하지 않아.

Proxy는 **전송 전에** 최악의 cache miss input ceiling + output bound 비용을 ledger에 append·fsync하고 reserve해. 알려진 비용과 모든 미정산 reservation을 합친 값이 cap을 넘으면 upstream으로 전달하지 않아. Usage/cache 누락·불일치, 다른 응답 model, timeout·transport error에는 reserve를 환급하지 않아. Schema가 틀린 proposal도 provider가 올바른 usage를 반환했다면 비용을 기록해.

가격·token ceiling이 실제 provider의 상한이라는 전제하에 적용하는 지출 guard야. 이 ceiling은 요청 텍스트 길이로 임의 추정하는 값이 아니고, live 실행 전에 공식 provider 계약으로 확인해야 해. Provider가 ceiling을 넘는 usage를 반환하면 batch를 중단하고 `provider_bound_violation`을 기록해. 이미 발생한 provider 과금을 되돌리거나 invoice 수준의 강제 한도를 보장하지 않아.

Quote는 source·UTC 시작/종료·`fixed_tier` 또는 `upper_bound`를 명시해. Window 밖에서는 전송하지 않아. Tier/공휴일 정책을 runner가 추측하지 않으므로 live smoke에서 해당 window와 단가를 다시 확인해야 해. `fixed_tier`와 완전한 usage/cache일 때만 `actual_cost_nano_usd`를 계산하고, 이는 공개 quote에 따른 계산이지 invoice 확인이 아니야. `upper_bound`는 실제 비용을 UNKNOWN으로 유지해. 모든 cache miss로 환산한 비용은 counterfactual 보조 지표야.

`known_budget_cost_nano_usd`는 알려진 부분합, `unknown_reserved_nano_usd`는 미확인 요청이 점유한 상한이야. UNKNOWN을 0으로 바꾸지 않아. 지출·quote 때문에 schedule이 끝나지 않으면 `complete_schedule=false`로 보고하고 해당 batch에서 채택을 주장하지 않아.

Ledger는 batch 디렉터리에서 새로 생성해. Crash한 batch의 ledger를 다시 읽어 재전송하는 resume 기능은 제공하지 않아. 미완료 평가 Run에 일반 CLI resume를 적용하지 않아. 완료된 journal은 독립적으로 보존해.

## 결과와 oracle

Fixture는 goal, visible files, 수정 가능한 상대 경로, 명시적으로 측정에서 제외할 generated directories, executable 목록과 독립 Python oracle를 가져. Domain 판단은 fixture에만 있어. Generated directory가 visible input을 숨기거나 workspace 전체를 제외하도록 설정할 수 없어.

Oracle 파일은 agent 종료 뒤 workspace 밖에 만들고, candidate workspace 복사본에서 실행해. Expected outcomes·hidden regression은 agent prompt·workspace·tool outputs에 넣지 않아. Symlink가 있는 candidate는 oracle 복사에서 거절해. 이 분리는 OS sandbox가 아니야. 허용한 process executable이 적대적으로 상위 경로를 탐색하는 상황의 격리는 별도 범위야.

최종 응답 계약은 JSON object의 `commands`와 `changed_files`야. Process argv/exit status는 durable output과 retained material recipe를 결합해서 얻고, 변경 파일은 실행 전후 inventory와 비교해. Oracle 성공·task completion·claim 일치·보호 파일 보존·측정 범위 밖 변경·중복 effect 시작은 따로 기록해. `false_claim`은 완료 후보의 명시적 claim 계약 불일치, `false_completion`은 완료 후보와 독립 oracle/파일 조건의 불일치야.

Adapter decode/rejection은 usage observations에서, Core plan 채택 개수와 model-call settlement는 journal에서 가져와. `ProposalDecoded`를 실행 완료로 간주하지 않아. 실패·거절·timeout trial을 모두 결과와 비용 집계에 포함해.

Binary는 private output directory에 복사해서 실행해. Binary, engine Rust sources·Cargo metadata, runner/oracle reader, executable, 입력 config/fixture 파일과 canonical config/fixture를 manifest에 hash로 고정해. 끝에서 재검사해 바뀐 batch는 무효 처리해. `source_unchanged`는 실행 전후 일치를 확인한 것이며 실행 도중 바뀌었다가 원래대로 돌아온 변조까지 증명하지는 않아.

## 로컬 계약 검사

```bash
cargo build -p xgen-cli --example planning_model_pilot
pilot_binary_dir=$(mktemp -d /tmp/xgen-pilot-binary.XXXXXX)
cp target/debug/examples/planning_model_pilot "$pilot_binary_dir/planning_model_pilot"
XGEN_PILOT_TEST_BINARY="$pilot_binary_dir/planning_model_pilot" \
  python3 -m unittest discover -s scripts/tests -p test_planning_model_eval.py
```

동시에 실행한 Cargo build가 원본 binary를 바꾸면 batch hash 검사가 실패할 수 있어. 위처럼 빌드 뒤 별도 경로에 복사한 binary를 입력해. 테스트는 실제 filesystem/process 도구를 실행하고 모델 응답만 로컬 HTTP fixture로 고정해. API를 호출하거나 과금하지 않아.

두 design family의 X0/X1/XN 6회 성공 경로, schema rejection 6회, 틀린 최종 claim 6회를 검사해. 합성 usage/quote에 따른 비용은 테스트 기대값이며 실제 모델 비용이 아니야. 추가로 unknown reserve 유지·중복 요청·quote expiry·token ceiling 위반·timeout process tree 정리·환경변수 분리를 검사해.

2026-10-05 최종 검증: frozen binary를 지정한 Python 전체 검사 59 passed, Rust workspace 659 passed/4 ignored, clippy·fmt·공개 문서 계약·diff whitespace PASS. 빌드가 원본 example binary를 덮어쓴 초기 배치는 hash 검사로 무효 처리했고 결과에 포함하지 않았어. 최종 검사는 별도 `/tmp` 경로에 복사한 binary로 다시 실행했어. Process 종료 검사의 `/proc` 파일 소멸 race도 수정한 뒤 전체 Python 검사를 다시 통과했어.

`evals/planning-model/design-smoke.json`은 runner 작동 확인용 두 design fixture야. [연구 계약](../research/2026-10-05-planning-size-model-pilot-research.md)의 최종 8 instance나 held-out validation을 대신하지 않아. `config.example.json`의 단가는 합성이고 quote window는 만료돼 있어 실제 전송에 쓸 수 없어.

## Live 실행 준비

```bash
python3 scripts/evaluate-planning-model.py \
  --binary /absolute/path/to/frozen/planning_model_pilot \
  --config /absolute/path/to/frozen-live-config.json \
  --fixtures /absolute/path/to/frozen-cases.json \
  --output /absolute/path/to/new-private-results \
  --upstream https://api.deepseek.com/v1
```

키는 로컬 환경에서만 설정해. Config·fixture·source·manifest에 저장하지 않아. Raw driver/oracle logs와 journal은 private results 디렉터리에 보존하고 공개 보고에는 비민감 집계만 사용해.

후속 [DeepSeek smoke와 사전 등록](deepseek-planning-pilot-smoke-2026-10-05.md)에서 개발용 live 18 trial과 usage/cache를 확인하고 120회 입력·config·quote를 고정했어. `--preregistration`으로 실행하면 등록한 binary/source/tool/input/schedule을 모델 I/O 전에 검사하고 등록 PATH를 사용해. Model/fingerprint 변경이나 확인 불가 응답은 비용/unknown reservation을 보존한 뒤 중단해. 최종 120회·validation 모델 비교는 아직 실행하지 않았어.
