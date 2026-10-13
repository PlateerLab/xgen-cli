# Completion gate 실제 모델 비교

2026-10-06. 범용 평가 작업. 엔진의 기본 동작은 바꾸지 않고, `--completion-contract`가 틀린 완료를 막는 효과와 실제 작업 성공·수정 효과를 분리해서 비교해.

## 방법과 사전 등록

동일 작업·도구·권한·예산·응답 schema에서 C0는 기존 Receipt 응답, C1은 필수 `verify` Receipt를 요구해. C1에는 provider의 completion-check 계약 prompt도 추가되므로 **prompt + gate 묶음의 비교**야. 순수 gate의 모델 행동 효과나 prompt만의 효과를 분리하지 않아.

DeepSeek `deepseek-flash`, `json_object`, thinking disabled, 최대 24 model turns를 사용해. 양쪽에 같은 host 관리 검증 프로그램과 `python3`를 허용해. 검증 프로그램은 모델 workspace 밖에 두고 executable catalog에 등록해. 입력 보존, 쓰기 범위, 독립 oracle로 실제 작업 성공을 판정하고, durable 완료 후보와 전체 Receipt 응답을 별도로 확인해. `verify`의 최초 nonzero 종료는 정상 process Receipt이며, 이후 실패→성공과 올바른 산출물이 함께 관측될 때만 검증 후 복구로 집계해.

숫자 JSON과 Unicode JSON 두 family에 각각 design·validation 입력을 하나씩 두고 case/repeat를 pair로 묶어 조건 순서를 균형 있게 섞었어. 반복 실행은 새 독립 입력으로 세지 않아. 최초 등록에는 24회, 후속 등록에는 다른 입력으로 16회를 지정했어. 통계적 유의성·비열등성·기본 채택을 주장하는 규모가 아니야. 완료 후보를 gate가 거절하면 해당 호출은 중단해. 숨은 retry나 거절 뒤 강제 수정은 추가하지 않았어.

- 최초 계약: `evals/completion-gate/{cases,config}.json`, `preregistration-2026-10-06.json`
- 후속 계약: `evals/completion-gate/followup/{cases,config}.json`, `preregistration-2026-10-06.json`
- 결과: `evals/completion-gate/results/2026-10-06/{initial,followup}/`

`REGISTERED_NOT_RUN`은 실행 전에 고정한 등록 상태이며 결과에 맞춰 수정하지 않아. source·binary·fixture·interpreter SHA-256, 모델 response identity, 일정, quote와 예산을 등록하고 실행 전·후 검증해.

## 최초 24회: gate 이전에 중단

| 지표 | C0 기본 | C1 gate |
|---|---:|---:|
| 실행 | 12 | 12 |
| 실제 성공·수락된 완료 | 2 | 0 |
| 틀린 완료 | 0 | 0 |
| gate 차단 | 0 | 0 |
| 검증 실패→성공 + 올바른 산출물 | 2 | 0 |
| 모델 호출 | 42 | 36 |
| quote 기준 비용 USD | 0.009161160 | 0.008159160 |

24회 중 22회에서 이미 존재하는 `result.json`을 `fs/write-atomic`의 `expectedDigest=null`로 덮어쓰려다 `EffectFailed`로 중단했어. null은 새 파일 생성 계약이고 기존 파일에는 현재 digest가 필요해. numeric·Unicode 양쪽의 retained recipe로 확인했어. 일부는 쓰려던 내용 자체는 맞았어도 파일 변경이 적용되지 않았어. 전체 실패 recipe는 `initial/failed-effect-evidence.json`에 보존했어.

이 단계는 실제 실패 결과이며 제외하거나 성공률에서 빼지 않아. C1의 틀린 완료 0은 완료 자체가 0인 결과이고, gate 효과를 측정할 완료 판단에 도달하지 못했어.

## 후속 등록의 변경

최초 결과를 보존한 뒤 기존 관측 입력을 재사용하지 않고 parity partition·bounded clamp·UTF-8 prefix offsets·ASCII edge trim 새 4개 작업을 등록했어. 최초 출력은 없는 상태이고, 생성에는 null, 덮어쓰기에는 현재 digest가 필요하다는 계약을 **양쪽 목표에 동일하게** 넣었어. 엔진·provider·평가 runner와 모델 설정은 그대로야. 새 사례에 대한 재현 가설을 시험하는 별도 단계이며 첫 결과와 합쳐 성공률 개선으로 해석하지 않아.

각 사례의 독립 oracle·host checker를 올바른 참조 산출물로 먼저 확인했어. 최초 출력이 없으면 checker가 실패하고 올바른 참조 결과에는 두 검사가 통과해. 검증 프로그램과 oracle은 모델 목표·workspace에 정답을 전달하지 않아. 실행 중 fixture나 prompt를 고치거나 실패한 trial을 재시도하지 않아.

| 지표 | C0 기본 | C1 gate |
|---|---:|---:|
| 실행 | 8 | 8 |
| 실제 작업 성공 | 0 | 0 |
| 수락된 완료 | 0 | 0 |
| 완료 응답 | 8 | 0 |
| 틀린 완료 | 8 | 0 |
| 검증 실패→성공 + 올바른 산출물 | 0 | 0 |
| 모델 호출 | 43 | 169 |
| gate 차단 | 0 | 4 |
| quote 기준 비용 USD | 0.010520238 | 0.098164320 |

후속 C0는 틀린 산출물에도 완료 후보를 확정했어. C1은 틀린 완료를 확정하지 않았고, 일부는 실패한 check 때문에 gate가 명시적으로 거절했어. 나머지 4회는 24 model turns를 사용한 뒤 완료 없이 중단했고 `trials.jsonl`에 그대로 포함했어. C1은 C0보다 모델 호출 169/43회, quote 비용 약 9.3배를 사용했어. 검증 프로그램은 실패 시 exit code만 반환하므로 이 실험은 기대/실제 차이를 제공하는 풍부한 feedback이나 후보 거절 뒤 repair 정책의 효과를 측정하지 않아. **차단 효과는 관측했지만 실제 수정·완료 개선은 입증하지 못했어.** 설계 입력과 validation 입력의 지표는 `analysis.json`에 따로 보존해.

최초 24회 + 후속 16회 = 40회 총 quote 비용은 **$0.126004878**야. 등록한 전체 $0.50 한도 이내이고 미정산 요청은 없어. 양 단계의 일정·source·binary·입력·관계 binding·비용 재계산 감사가 통과했어.

기본 적용은 보류하고 선택 옵션을 유지해. 다음 범용 작업은 실패한 검증의 원인과 기대/실제 차이가 bounded feedback으로 모델에 전달되는지 두 family에서 확인하고, 검증 실패→수정→재검증→완료를 별도 held-out 입력에서 평가하는 거야. 최종 후보 거절 뒤 재개·retry 정책은 별도 설계와 예산 계약이 필요해. 특정 숫자 계산이나 Unicode 규칙을 엔진 prompt에 박아 넣지 않아.

Python 73개 검사(실제 bridge 및 두 작업의 실패 journal mutation 포함), `cargo clippy --workspace --all-targets -- -D warnings`, fmt, public docs 검사, diff 검사를 통과했어. 이번 변경은 평가 bridge·runner·분석·자료이고 production 엔진 변경은 없어.


## 재현과 감사의 범위

원본 workspace·journal·Receipt·sidecar·실행 binary는 로컬 private archive에 보존해. 공개 결과에는 manifest, trial 지표, 요청별 usage와 quote ledger, 분석, 실행 당시 runner, 가격 snapshot을 보존해.

```bash
python3 scripts/analyze-completion-gate.py \
  --results ~/.local/share/xgen/evaluations/completion-gate-initial-2026-10-06 \
  --preregistration evals/completion-gate/preregistration-2026-10-06.json \
  --output /tmp/completion-gate-initial-analysis.json
```

후속 실행은 `completion-gate-followup-2026-10-06` archive와 `followup/preregistration-2026-10-06.json`을 사용해.

분석은 순서·중복 시작·이전 승인 binding·사용 횟수·retained material·성공 output/Receipt의 관계를 검사해. 실패 effect는 승인과 material을 그대로 요구하고, 같은 step의 durable 실패 및 성공 output/Receipt 부재를 확인해. 성공 Receipt를 모든 시작 effect에 요구하는 기존 분석기는 정상적인 `EffectFailed`에도 실패하므로 이 분석기에 실패 상태의 coverage를 추가했어. cryptographic digest/signature의 독립 재계산이나 failure evidence 내용 검증, OS sandbox 증명은 하지 않아. 검증 프로그램의 판정이 임의 작업의 의미적 진실을 보장하는 것도 아니야.

두 다른 실패 작업의 journal 복사본에서 승인 action digest 변조, 실패 step 변조, 실패 이벤트 제거를 각각 거절하는 mutation 검사를 실행해. private archive 기반 검사는 `XGEN_GATE_AUDIT_FIXTURES`로 명시적으로 켜야 해.

비용은 [DeepSeek 공식 가격표](https://api-docs.deepseek.com/quick_start/pricing/)의 UTC off-peak quote와 보존한 cache/input/output usage로 독립 재계산해. snapshot SHA-256은 등록 파일에 고정했고 public `initial/pricing.html`에 보존해. 공급자 청구서와 대사한 금액은 아니야.
