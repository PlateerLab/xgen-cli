# 실제 모델의 계획 크기 비교: 조사와 구현 준비

- 조사일: 2026-10-05, Asia/Seoul
- 구현 기준: `f6576aa` ([모델 없는 probe](../development/planning-boundary-probe-2026-10-05.md) 완료)
- 변경 분류: 범용 조사·평가 설계. 사례별 engine 규칙 없음.
- 상태: 문헌·공개 구현·현재 provider 경로 조사 완료. 후속 [개발용 DeepSeek smoke](../development/deepseek-planning-pilot-smoke-2026-10-05.md) 18 trial 완료. 최종 120회·validation·외부 엔진 비교는 NOT_RUN.
- 기존 계획: [planning boundary 평가](2026-10-05-planning-boundary-evaluation.md)
- 근거 inventory: [PDF·code·pricing snapshot hash](2026-10-05-planning-size-model-pilot-sources.json)

## 판단

`INFERENCE`: 같은 엔진에서 proposal 크기 1과 4만 바꾸는 pilot은 진행할 근거가 있어. 단, 현재 constrained provider prompt가 한 Step만 요구하므로 먼저 평가용 공통 planning contract를 마련해야 해. Production 기본값은 유지하고, 비교군의 prompt·schema·검사·request profile에 상한이 일관되게 반영돼야 해.

ReWOO·LLMCompiler의 전체 프레임워크를 넣는 작업은 이번 비교와 분리해. 두 방법의 future-output substitution은 현재 xgen의 concrete literal arguments와 다르고, LLMCompiler의 parallelism·streaming 효과를 직렬 batching 효과로 설명할 수 없어.

## 근거의 범위

`VERIFIED`는 원문·공개 코드·현재 repo에서 확인한 계약, `RECALCULATED`는 공개 숫자의 재계산, `AUTHOR_CLAIM`은 재현하지 않은 논문 결과, `INFERENCE`는 적용 판단, `UNKNOWN`은 확인되지 않은 결과야.

ReWOO 25쪽, LLMCompiler camera-ready 22쪽, AI Agents That Matter arXiv v1 33쪽의 본문·부록을 extraction으로 검토했어. 큰 문서의 추출에는 delegate를 썼고, 아래 핵심 방법·표·한계는 원문 text와 공개 code에서 다시 확인했어. 첫 통합 extraction은 잘려서 근거로 채택하지 않았고 각 논문을 별도로 다시 추출했어. 원 실험을 재현하거나 모든 metric의 raw-data 집계를 감사한 것은 아니야.

ReWOO는 Xu·Peng·Lei·Mukherjee·Liu·Xu의 arXiv:2305.18323v1, LLMCompiler는 Kim·Moon·Tabrizi·Lee·Mahoney·Keutzer·Gholami의 ICML 2024 논문이야. AI Agents That Matter는 Kapoor·Stroebl·Siegel·Nadgir·Narayanan의 arXiv:2407.01502v1을 사용했어. OpenReview의 후속 최종 PDF는 403으로 받지 못해 버전을 섞지 않았어. 이 자료들은 최신 Google 논문 조사라는 의미가 아니라 이번 비교 설계의 직접적인 선행 방법·평가 근거야.

공개 code HEAD는 inventory의 commit에 고정했어. 논문 실험과 정확히 같은 commit인지는 UNKNOWN이야. ReWOO와 LLMCompiler 실행 경로 일부를 확인했고, agent-evals는 repository metadata만 확인했어. 코드를 설치·실행하거나 논문의 성능을 재현하지 않았어. 직접적인 후속 비판·재현을 검색했지만 이 메모에서 검증할 수 있는 동일 조건의 proposal-size ablation은 찾지 못했어. 검색 결과의 부재를 연구가 없다는 증명으로 해석하지 않아.

## 문헌에서 가져올 부분과 제외할 부분

| 자료 | 원 방법·확인한 한계 | xgen 적용 |
|---|---|---|
| [ReWOO, arXiv v1](https://arxiv.org/abs/2305.18323) | Planner·Worker·Solver 분리, `#E` 변수 대입. Sec.4는 환경을 모르는 탐색에서 선계획이 불리할 수 있음을 인정해. | 알려진 literal 작업을 묶는 가설만 가져와. 변수 대입·Solver 분리·fine-tuning은 별도 조건이야. |
| [LLMCompiler, ICML 2024](https://proceedings.mlr.press/v235/kim24y.html) | DAG planner, dependency 결과 대입, async scheduler, streaming, replanning. Appendix D는 temperature 0에서도 3회 반복했다고 명시해. | Dependency·관찰 경계를 나눠 평가해. Parallel executor·streaming·추가 joiner는 이번 비교에 넣지 않아. |
| [AI Agents That Matter, arXiv v1](https://arxiv.org/abs/2407.01502) | 비용과 정확도의 공동 평가, 개발 비용과 inference 비용 구분, 일반화 수준에 맞는 holdout 요구. | 실패·usage missing을 보존하고 design과 validation을 분리해. 반복 trial을 새로운 독립 task로 세지 않아. |

`VERIFIED`: 공개 ReWOO code의 [PWS.run](https://github.com/billxbf/ReWOO/blob/9cd0283043ff4be0c9d614fda2789d143ca6ffd1/algos/PWS.py#L29)은 Planner→Worker→Solver를 연결하고, [L101–104](https://github.com/billxbf/ReWOO/blob/9cd0283043ff4be0c9d614fda2789d143ca6ffd1/algos/PWS.py#L101)는 `#E`를 앞선 evidence로 치환해. xgen의 existing dependency는 이런 입력 값 대입 기능이 아니야.

`VERIFIED`: 공개 LLMCompiler의 [TaskFetchingUnit.schedule, L124–135](https://github.com/SqueezeAILab/LLMCompiler/blob/a00c9d35507507da70e8c637eee64efc8c1857ae/src/llm_compiler/task_fetching_unit.py#L124)는 dependency를 검사하고 `asyncio.create_task`로 실행해. [LLMCompiler의 설정](https://github.com/SqueezeAILab/LLMCompiler/blob/a00c9d35507507da70e8c637eee64efc8c1857ae/src/llm_compiler/llm_compiler.py#L55)은 planner streaming과 max_replans를 별도로 받아. 이 효과를 제거한 동일 엔진 비교가 필요해.

### 숫자에서 확인한 주의점

`AUTHOR_CLAIM`/`RECALCULATED`: ReWOO Table 2의 HotpotQA tokens는 9795.1→1986.2라 비율은 4.932배야. Semantic Acc는 40.8→42.4로 +1.6 percentage points, 상대 증가율은 약 3.92%야. 반면 EM은 32.2→30.4로 -1.8 points야. 하나의 headline으로 최종 답의 모든 정확도가 개선됐다고 말할 수 없어. [원 논문 Table 2](https://arxiv.org/pdf/2305.18323v1)

`AUTHOR_CLAIM`/`RECALCULATED`: LLMCompiler Table 2의 Movie Recommendation 비용 비율은 20.46/3.04=6.730배야. Appendix C.1의 ParallelQA는 streaming만 바꿔도 21.72/16.69=1.301배 차이가 나. 이 표들은 저자 실험이고 xgen에서 재현한 수치가 아니야. 같은 paper의 반올림된 숫자에 기반한 재계산으로만 사용해. [원 논문 Table 2·C.1](https://raw.githubusercontent.com/mlresearch/v235/main/assets/kim24y/kim24y.pdf)

## 현재 구현에서 확인한 준비 상태

| 상태 | 사실 | 최소 구현 면 |
|---|---|---|
| ALREADY_PRESENT | Core는 multi-step proposal·existing/proposed dependency와 직렬 frontier 실행을 지원해. | 엔진·scheduler 변경 불필요. |
| ALREADY_PRESENT | Provider는 call ID·request digest·elapsed time·decode outcome·usage observer를 제공해. | 현재 observer를 평가 ledger에 연결해. |
| ALREADY_PRESENT | Usage decoder는 native/standard cached input의 일치, total=input+output, reasoning≤output을 검사하고 미확인을 None으로 남겨. | 비용 집계에서 None을 0으로 바꾸지 않아. |
| ADOPT_WITH_GATES | Constrained prompt는 정확히 한 Step과 빈 dependsOn을 요구해. 탐색 또는 process 도구가 있으면 이 prompt가 선택돼. | 평가용 공유 prompt·proposal 상한을 명시하고 profile로 고정해. |
| ADOPT_WITH_GATES | 최종 응답 명령·변경 파일을 실제 journal/output과 대조하는 평가가 있어. | 새 fixture의 명시적 응답 계약과 독립 artifact/test oracle에 재사용해. |
| DO_NOT_ADOPT | 변수 치환·streaming·parallel scheduler·별도 model joiner를 함께 넣으면 여러 변수가 바뀌어. | 별도 ADR·ablation 전에는 도입하지 않아. |

`VERIFIED`: [provider prompt, L50–60](../../crates/xgen-provider-openai/src/lib.rs)는 한 Step만 요구해. [CLI composition, L983](../../crates/xgen-cli/src/composition.rs)에서 workspace discovery 또는 process 사용 시 constrained 경로를 선택하고 [planner_config, L1859](../../crates/xgen-cli/src/composition.rs)에서 해당 option을 적용해. Host constraint 하나를 추가하는 것만으로 XN 비교를 시작할 수 없어.

`VERIFIED`: [profile descriptor, L383–417](../../crates/xgen-provider-openai/src/lib.rs)는 prompt/schema digest·model·thinking·limits를 고정해. 평가 크기를 prompt/schema에 반영하면 이 결합을 유지하고 기존 resume를 바꾸지 않아야 해. 상한만 다르고 나머지 안전·literal·관찰 규칙이 같은 새로운 평가 profile로 두는 게 제안이야.

`VERIFIED`: [observer 경로, L935–954](../../crates/xgen-provider-openai/src/lib.rs), [usage decoder, L41–86](../../crates/xgen-provider-openai/src/usage.rs), [최종 응답 대조, L109–126](../../scripts/evaluate-projects.py)를 확인했어. `ProposalDecoded`는 Core가 계획을 채택하거나 실행 완료했다는 뜻이 아니야. 이후 admission rejection도 별도로 집계해야 해.

## 비용과 cache 측정 계약

`VERIFIED`: 2026-10-05 공식 pricing page의 `deepseek-flash` alias는 DeepSeek-V4.1-Flash로 안내돼. 1M tokens당 USD 기준은 아래와 같아. Web reader가 실패해 직접 HTTPS로 받은 page를 snapshot hash로 기록했어. 실행 전에 다시 확인해야 해. [DeepSeek Models & Pricing](https://api-docs.deepseek.com/quick_start/pricing/)

| 단가 | peak | off-peak |
|---|---:|---:|
| cached input | 0.006 | 0.003 |
| uncached input | 0.30 | 0.15 |
| output | 1.20 | 0.60 |

공식 정책은 UTC weekday 시간 구간과 중국 공휴일 예외를 포함해. KST 시각이나 weekday만으로 tier를 추정하지 않아. 조건별 실행을 균형 있게 섞고 동일 tier의 block으로 묶어. Alias·응답 model·server version/fingerprint가 달라지면 기록하고, 의미가 확인되지 않은 버전 변경은 조건을 섞지 않고 중단해. [가격 정책](https://api-docs.deepseek.com/quick_start/pricing/), [Chat Completions 응답 계약](https://api-docs.deepseek.com/api/create-chat-completion/)

`INFERENCE`: 입력 I, cached input H, output O와 단가 p로 `((I-H)*p_miss + H*p_hit + O*p_output)/1e6`을 계산해. Reasoning tokens는 O에 포함된 subset이므로 추가 과금하지 않아. API-reported usage와 공개 단가의 계산이며 invoice 대조는 별도야.

같은 peak 단가에서 I=1M·O=100k인 가상 요청은 H=0이면 $0.420, H=1M이면 $0.126이야. 이는 호출 수가 같아도 비용이 달라질 수 있다는 재계산 예시이며 실제 pilot 사용량이 아니야. Cache가 달라진 실비와 같은 tier에서 모두 cache miss로 환산한 비용을 함께 보고해. Cache miss 환산은 counterfactual이고 실제 청구액이 아니야.

Usage·cache·적용 tier가 미확인이면 실제 비용은 UNKNOWN으로 둬. Schema rejection·실패·timeout·unknown call을 무료로 처리하지 않아. 알려진 사용량 합계와 미확인 요청 수·상한을 별도로 남겨. 총 비용을 정확히 계산할 수 없으면 기존 15% 비용 채택 gate도 UNKNOWN이야.

## 구체적인 pilot 설계

기존 8 instance × X0/X1/XN × 5회=120 normal trials를 유지해. 최종 fixture 내용·hash·단가·전체 지출 상한은 아직 고정하지 않았으므로 실행 가능한 preregistration 완료 상태는 아니야.

| 조건 | Planner contract | 역할 |
|---|---|---|
| X0 | 기존 production prompt/profile 그대로 | 현실성 reference. Step 개수가 1이라고 가정하지 않고 측정해. |
| X1 | 공통 평가 prompt + maxProposalSteps=1 | 직접 비교군 |
| XN | 같은 평가 prompt + maxProposalSteps=4 | 직접 비교군. 최대 4개이지 반드시 4개가 아니야. |

X0와 XN의 차이는 prompt 변경을 포함하므로 순수한 계획 크기 효과가 아니야. 주 비교는 X1↔XN으로 두고, profile 차이는 선언된 Step 상한에서만 나오도록 검사해. Output token·tool/model call·step·context bound, tool catalog, approval 범위, retry·timeout·thinking 옵션은 같게 두고 모든 도구 실행은 직렬로 유지해.

상한을 넘는 proposal은 줄여 실행하지 않아. 모델에게 먼저 상한을 전달하고 host가 검증해. Future-output 문자열 참조나 placeholder는 허용하지 않아. 이후 관찰로 결정되는 path·value는 Receipt-bound output을 받은 다음 plan에서 구체화해. 각 concrete invocation의 승인·material binding은 유지해.

Design은 명시된 여러 입력의 읽기·artifact 생성 2개와 작은 Rust project 진단·수정 2개로 두고, validation은 구조가 다른 manifest/artifact 작업 2개와 Python CLI 진단·수정 2개로 둬. 기존 설계 사례의 이름·언어·값만 바꿔 validation으로 쓰지 않아. Hidden regression과 expected artifact는 agent workspace·prompt·tool outputs에 넣지 않아. 평가용 domain 규칙은 user task/fixture에만 있고 engine/prompt에 넣지 않아.

최종 응답은 fixture가 정한 명시적 JSON claim을 독립 oracle와 비교해. 실제 command·exit status·변경 파일은 journal과 durable output에서 가져와. Command 성공만으로 의미적 수정 완료를 판단하지 않고 외부 hidden test·artifact 상태를 추가로 검사해. 최종 claim 정확도와 작업 완료는 별도 지표야.

각 instance/repeat를 paired block으로 두고 X0/X1/XN 순서를 사전 seed로 균형 있게 섞어. 매 trial은 새 isolated workspace·Run·journal로 실행하고 binary copy·source/config/fixture hash를 고정해. 실패 trial을 새 성공 trial로 교체하거나 best-of-N을 쓰지 않아. 개발용 smoke 사용량은 fixed development cost로 별도 집계하고 validation trial 비용과 섞지 않아.

4 validation instance의 5회 반복은 4개의 독립 task라는 점을 유지해. Pilot은 descriptive paired 결과이고 일반적 통계적 superiority나 정확도 non-inferiority의 증명이 아니야. Task별 완료 수·false claim·실비·cache miss 환산 비용·총 예약/unknown/rejected call·wall latency를 함께 제시해. Failed trial 비용도 포함한 전체 집계를 먼저 보고, 성공 trial만의 비용은 보조 지표로 둬.

기존 gate는 최소 두 validation family에서 XN 완료가 X1보다 적지 않고, 새로운 unsafe duplicate·미승인 실행·false completion이 0이며, validation median 모델 비용이 15% 이상 낮아지는 것이야. Pilot gate 통과는 더 큰 확인 실험의 근거로만 해석해. Usage가 없거나 한 조건에서 전부 미완료라 낮은 비용이 나온 경우 default를 채택하지 않아.

## 다음 구현 단위와 실행 전 gate

2026-10-05 구현 상태: 1번의 opt-in shared profile·상한 local gate와 모델 없는 계약 검사를 추가했어. [구현 계약과 검증 범위](../development/evaluation-planning-profile.md)를 참고해. Runner·실제 API smoke·120 trial은 아직 실행하지 않았어.

후속 구현 상태: 2번 [model pilot runner](../development/planning-model-pilot-runner.md)를 구현해 실제 driver·filesystem/process·독립 oracle·usage·비용 reservation을 연결했어. 로컬 고정 모델 응답으로 계약만 검사했고, live API smoke·실제 모델 비교·최종 120 trial은 아직 실행하지 않았어.

후속 live 상태: [개발 smoke·사전 등록](../development/deepseek-planning-pilot-smoke-2026-10-05.md)에 연결 6회와 design 12회를 기록했어. 작업 oracle 18/18, 최종 claim 포함 17/18이며 XN 명령 chronology 불일치 1회는 보존했어. Validation 모델 실행은 0회이고 최종 120회 조건은 고정했어. 성능 개선이나 기본값 변경은 채택하지 않았어.

1. **범용:** 평가용 shared planning prompt·maxProposalSteps 상한을 provider profile에 결합해. 기본 production profile/resume는 유지해. 상한 차이·초과 거부·관찰 뒤 계획·dependency·profile 불일치를 모델 없는 transport fixture로 검사해.
2. **범용:** 실제 CLI/driver와 usage observer·독립 oracle를 연결하는 model pilot runner를 만들어. 고정 binary copy, 실패/timeout 기록, request/decode/admission 분리, tier/cache 비용과 지출 상한을 포함해. 재계획 효과를 보기 위해 기존 scripted sink가 아닌 실제 filesystem/process 경로를 써.
3. **범용:** Design fixture를 먼저 고정하고 작은 실제 API smoke로 model 응답 ID·usage/cache·schema 호환성과 필요한 호출 bound를 확인해. Manifest에 전체 비용 한도와 unknown-call reserve를 고정한 다음 validation fixture/hash와 config를 잠그고 120 trial을 실행해. 현재는 이 모델 호출을 실행하지 않았어.
4. **범용:** 결과로 planner 기본값 변경을 판단해. Auto reconciliation·typed dataflow·parallel 실행·memory는 별도 작업이야.

전체 비용 guard는 단순히 끝난 요청의 비용 합계만 검사해서는 hard cap이 아니야. 다음 요청의 보수적인 최대 요금을 먼저 reserve하고, unknown usage의 reserve는 환급하지 않는 계약이 필요해. 비용 bound와 model-call/tool-call 공통 budget이 동시에 120 trial을 수용하는지는 smoke 이후 확인할 항목이야. 수용하지 못하면 조건별 budget을 다르게 낮추지 말고 사전에 pilot 규모를 다시 등록해.

## 후속 본실험 결과

2026-10-05 [등록한 120회 pilot](../development/planning-model-pilot-results-2026-10-05.md)을 완료했어. 작업 oracle 120/120, 응답 계약 포함 94/120이야. Validation XN 비용 median은 X1 대비 24.5% 낮았지만 응답 형식 위반 1회가 남아 gate는 FAIL이고 production 기본값을 유지해. 위 미실행 표시는 각 구현·등록 시점의 기록이야.
