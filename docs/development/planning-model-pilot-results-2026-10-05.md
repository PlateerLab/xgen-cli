# DeepSeek 계획 크기 120회 pilot 결과

2026-10-05. 범용 평가. **120회 실행을 완료했고, production 기본값은 유지해.** 작업 oracle는 120/120이지만 최종 응답 계약까지 포함한 통과는 94/120이야. XN의 validation 비용 median은 X1 대비 24.5% 낮았지만, XN 자체에서도 validation 형식 위반 1회가 남아서 등록한 gate는 FAIL이야.

[개발 smoke와 등록](deepseek-planning-pilot-smoke-2026-10-05.md), [사전 등록 원본](../../evals/planning-model/preregistration-2026-10-05.json), [분석 계약](planning-model-pilot-analysis.md), [전체 결과](../../evals/planning-model/results/2026-10-05/registered-pilot/analysis.json)를 함께 봐. 등록 파일의 `REGISTERED_NOT_RUN`은 등록 시점 상태이며 hash를 보존하기 위해 수정하지 않았어. 실행 결과는 별도 파일이야.

## 실행 조건과 보존

8개 서로 다른 사례를 조건별로 5회 반복했어. Design 4개·validation 4개이며, validation 20회는 독립 task 20개가 아니야. 등록 seed `202610052`의 120회 순서를 그대로 실행했어. 개발 smoke 18회는 이 집계에서 제외했어. 실패를 재실행하거나 성공한 반복만 고르지 않았어.

X0는 기존 production prompt, X1은 공통 평가 prompt + 최대 1 Step, XN은 같은 평가 prompt + 최대 4 Step이야. 주 비교는 X1↔XN이야. X0↔XN은 prompt도 달라서 순수한 계획 크기 효과로 읽으면 안 돼. 모든 도구 실행은 직렬이고 미래 tool output 치환·parallel 실행은 없어.

모델은 `deepseek-flash`, `json_object`, thinking disabled로 고정했고, 515개 upstream 요청이 모두 HTTP 200으로 정산됐어. 모델/fingerprint, binary, source, config, fixture, 실행 순서가 등록과 일치했어. UNKNOWN 비용·token bound 위반·timeout·자동 retry는 0이야. [manifest](../../evals/planning-model/results/2026-10-05/registered-pilot/manifest.json)와 [summary](../../evals/planning-model/results/2026-10-05/registered-pilot/summary.json)에 원자료가 있어.

전송 전에 보수적 최대 비용을 reserve하는 $2 guard를 사용했어. 계산된 전체 모델 비용은 **$0.141032598**이며, 개발 smoke 비용 $0.021827646과 별도야. [DeepSeek 공식 가격](https://api-docs.deepseek.com/quick_start/pricing/)의 등록된 off-peak 요율과 provider usage/cache로 계산한 값이고, 실제 청구서와 대조한 금액은 아니야. 가격·token bound 계약을 전제로 하는 guard여서 provider 청구 상한을 직접 설정한 것은 아니야.

실행 당시 [runner snapshot](../../evals/planning-model/results/2026-10-05/registered-pilot/runner-at-execution.py), [analysis snapshot](../../evals/planning-model/results/2026-10-05/registered-pilot/analysis-at-execution.py), [trial JSONL](../../evals/planning-model/results/2026-10-05/registered-pilot/trials.jsonl), [cost ledger](../../evals/planning-model/results/2026-10-05/registered-pilot/cost-ledger.jsonl)을 보존했어. SQLite·바이너리·최종 후보 파일은 로컬 `/home/son/.local/share/xgen/evaluations/planning-model-2026-10-05`에 보존했고, journal/binary hash가 원본과 일치해. 생성 build 디렉터리는 후보 복사에서 제외했어. 공개 hash만으로 비공개 journal 내용을 독립 검증할 수는 없어.

## 조건별 결과

`accepted`는 oracle·응답 계약·보호 파일·수정 범위·중복 effect 검사를 모두 통과한 수야. 실패한 실행의 비용도 아래 median과 총액에 포함해.

| Split | 조건 | 작업 oracle | Accepted | 응답 계약 실패 | 모델 호출 합계 | Trial 비용 median | 모델 비용 합계 |
|---|---|---:|---:|---:|---:|---:|---:|
| design | X0 | 20/20 | 20/20 | 0 | 100 | $0.001353834 | $0.027619350 |
| design | X1 | 20/20 | 14/20 | 6 | 100 | $0.001442799 | $0.028438086 |
| design | XN | 20/20 | 15/20 | 5 | 68 | $0.001027149 | $0.020103618 |
| validation | X0 | 20/20 | 20/20 | 0 | 90 | $0.001259409 | $0.022480212 |
| validation | X1 | 20/20 | 6/20 | 14 | 90 | $0.001302033 | $0.023754426 |
| validation | XN | 20/20 | 19/20 | 1 | 67 | $0.000982632 | $0.018636906 |

Validation XN의 모델 호출은 X1의 90회에서 67회로 25.6% 줄었고, 비용 median은 $0.001302033 → $0.000982632로 24.5% 줄었어. Cache를 모두 miss로 환산한 보조 median 감소는 31.6%야. 전체 515회 요청을 모두 miss로 환산한 값은 $0.517766550야. 이것은 실제 청구액이 아니야.

XN의 validation latency median은 5.90초, X1은 7.93초야. Design에서는 XN 11.34초, X1 10.40초로 방향이 반대였어. 모델 호출이 줄었다고 모든 작업이 빨라진다고 결론내리지 않아. Cache와 응답 변동도 남아 있어.

## 최종 응답 오류의 의미

총 26회 응답 계약 실패 중 21회는 요구한 JSON 대신 설명문을 반환한 형식 위반이고, 5회는 실제 process 실행 순서를 다르게 적은 경우야. 형식 위반을 모두 사실 조작으로 해석하지 않아. 이번 `false_claim`은 strict 계약 위반 지표야.

X1은 design 형식 위반 6회와 validation 형식 위반 14회였어. XN은 서로 다른 Rust design 사례 두 개에서 명령 순서 불일치 5회, validation `log-cli`에서 형식 위반 1회였어. 작업 oracle와 수정 범위는 모두 통과했으며, 잘못된 작업 완료 주장을 뜻하는 `false_completion`은 0회야.

명령 순서 불일치 예시는 실제 기록의 실패 `cargo test` → 성공 `cargo build` → 성공 `cargo test`를 응답에서 실패 test → 성공 test → 성공 build로 적은 경우야. Scheduler가 독립 Step을 실행한 순서와 최종 응답이 적은 순서는 별도로 비교했어. [오류 원문과 실제 명령](../../evals/planning-model/results/2026-10-05/registered-pilot/analysis.json)의 `claim_failures`에 기록했어.

X0는 40/40인데 평가용 X1은 20/40이어서, 새 평가 profile이 기존 production보다 응답 계약에서 약해진 것도 확인됐어. XN이 X1보다 좋아 보이는 결과만으로 production 개선이라고 주장하지 않아. 원인이 prompt의 특정 문장인지, 구조화 summary 계약 부재인지, 모델의 변동인지 분리한 추가 실험은 아직 없어.

## 승인·Receipt와 gate

120개 journal, 452개 effect 실행에서 선행 plan/intent/authorization, grant 소비, invocation/material/recipe, output/Receipt의 관계가 일치했어. 중복 effect 시작은 0이야. 관측한 모든 실행 시작에 사전 승인 binding이 있었어. 이는 관계 검사이며 모든 digest·signature의 독립 재계산이나 OS sandbox 증명은 아니야. 프로세스가 host의 다른 파일을 읽지 못한다고 주장하지 않아.

| 등록 조건과 무결성 대조 | 결과 |
|---|---|
| `complete_and_sources_unchanged` | PASS |
| `cost_known` | PASS |
| `validation_completion_per_family_noninferior` | PASS |
| `no_duplicate_effect_starts` | PASS |
| `no_validation_false_claims` | FAIL |
| `no_validation_false_completions` | PASS |
| `validation_median_cost_reduction_at_least_15_percent` | PASS |
| `journal_binding_audit_passed` | PASS |
| `no_all_incomplete_cost_advantage` | PASS |
| `registered_schedule_exact` | PASS |
| `response_identity_unchanged` | PASS |
| `preregistration_matches_manifest` | PASS |
| `cost_ledger_reconciled` | PASS |

최종 판정은 **FAIL, 기본값 유지**야. XN은 validation 두 family에서 accepted가 X1보다 적지 않고 비용 기준도 통과했지만, validation 응답 계약 실패 0회 조건을 넘지 못했어. 통과했더라도 큰 확인 실험의 근거로만 쓸 계획이었어.

## 사례별 원자료 요약

| 사례 | 조건 | Accepted | 모델 호출 합계 | Trial 비용 median |
|---|---|---:|---:|---:|
| design-adjacent-runs | X0 | 5/5 | 30 | $0.002008152 |
| design-adjacent-runs | X1 | 4/5 | 30 | $0.002029500 |
| design-adjacent-runs | XN | 2/5 | 19 | $0.001390398 |
| design-half-open-range | X0 | 5/5 | 30 | $0.001963068 |
| design-half-open-range | X1 | 5/5 | 30 | $0.001990416 |
| design-half-open-range | XN | 3/5 | 19 | $0.001439814 |
| design-literal-bundle | X0 | 5/5 | 20 | $0.000785550 |
| design-literal-bundle | X1 | 2/5 | 20 | $0.000819882 |
| design-literal-bundle | XN | 5/5 | 10 | $0.000481332 |
| design-observed-settings | X0 | 5/5 | 20 | $0.000770400 |
| design-observed-settings | X1 | 3/5 | 20 | $0.000767832 |
| design-observed-settings | XN | 5/5 | 20 | $0.000765132 |
| validation-log-cli | X0 | 5/5 | 25 | $0.001441584 |
| validation-log-cli | X1 | 0/5 | 25 | $0.001574082 |
| validation-log-cli | XN | 4/5 | 17 | $0.001090896 |
| validation-numeric-cli | X0 | 5/5 | 25 | $0.001377984 |
| validation-numeric-cli | X1 | 1/5 | 25 | $0.001406532 |
| validation-numeric-cli | XN | 5/5 | 15 | $0.001011132 |
| validation-routed-manifest | X0 | 5/5 | 25 | $0.001109286 |
| validation-routed-manifest | X1 | 1/5 | 25 | $0.001187934 |
| validation-routed-manifest | XN | 5/5 | 20 | $0.000951450 |
| validation-tsv-manifest | X0 | 5/5 | 15 | $0.000528600 |
| validation-tsv-manifest | X1 | 4/5 | 15 | $0.000543282 |
| validation-tsv-manifest | XN | 5/5 | 15 | $0.000568332 |

Case/repeat별 X1↔XN paired 값도 analysis의 `paired_blocks`에 있어. 이 4개의 validation 사례는 이제 관찰한 데이터라서 다음 수정의 새 held-out 검증으로 재사용하지 않아.

## 검증과 다음 작업

Python 검사 70개 PASS. Analysis 검사 9개 중 실제 development SQLite를 사용하는 두 검사는 private 자료를 제공해 실행했어. 서로 다른 두 사례에서 grant action·material·Receipt invocation을 훼손하거나 intent를 제거하면 audit가 실패했어. `cargo fmt --all --check`, 공개 문서 계약 검사도 PASS야. Rust 실행 코드는 이번 결과 수집에서 수정하지 않았으므로 기존 659개 workspace 검사를 다시 실행한 것으로 기록하지 않아.

보조 검사에서 Linux 종료 상태 `X`를 `Z`와 다르다고 실패시킨 경합을 한 번 관측했어. 검사만 `Z`·`X`·`x` 또는 `/proc` 소멸을 허용하도록 수정했고, 전체 70개를 다시 통과했어. 이 검사 파일은 등록된 model 실행 source에 포함되지 않고, runner·binary·fixture는 변경하지 않았어.

다음 작업은 범용 **최종 응답 계약과 evidence projection** 설계야. 선택적 응답 schema를 host 설정·request profile에 명시하고, 명령/exit status/실행 순서는 Receipt에서 파생하도록 하는 방향을 검토해. 내용의 의미적 정답은 여전히 task별 독립 oracle로 검사해야 해. 도메인 규칙을 engine에 넣거나 이번 사례 이름에 맞춘 분기를 추가하지 않아.

형식 강제와 관찰 사실 연결은 다른 수정이므로 따로 평가해. 지금 확인한 서로 다른 사례의 실패를 regression으로 보존하되, 수정 전에 새로운 입력 계약과 두 family 이상의 design/held-out 사례를 따로 고정해야 해. 새 prompt나 schema가 기존 response/resume digest와 어떻게 공존할지도 설계 항목이야. 이번에는 그 수정이나 production 배포를 수행하지 않았어.

후속 작업에서 선택적 [최종 응답 계약과 Receipt projection](final-response-contract.md)을 구현했어.
별도 새 사례 24회의 형식·실행 기록은 모두 일치했지만 작업 oracle는 14/24였어.
이전 120회의 결과와 등록 gate는 그대로 보존하고, 두 실험의 정확도를 직접 비교하지 않아.
