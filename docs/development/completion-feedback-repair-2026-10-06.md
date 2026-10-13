# 검증 진단으로 수정·재검증하기

2026-10-06. 범용 변경. 기존 process stdout·stderr와 Receipt 경로를 그대로 사용하고, host 검증 프로그램이 실패 원인을 제한된 JSON으로 반환할 수 있게 했어. 완료 gate와 승인·복구·예산 정책은 바꾸지 않아.

## 원인 확인

앞선 [completion gate 비교](completion-gate-live-pilot-2026-10-06.md)의 UTF-8 prefix offset·integer parity partition 두 작업에서 실패한 process tool output을 확인했어. exitCode는 1이고 stdout·stderr는 모두 빈 문자열이었어. truncation도 false였어. 모델이 실패 원인을 받을 자료 자체가 없었고, 같은 틀린 결과를 재검증하거나 완료를 제안했어.

`build_planning_tool_outputs`는 검증된 tool output을 그대로 다음 planning context에 넣어. 진단 전달을 위해 별도 엔진 채널이나 자동 retry를 만들 필요는 없었어. tool output은 여전히 비신뢰 데이터이고 지시·권한으로 취급하지 않아.

## 재사용 가능한 검증 프로그램

`scripts/check-json-result.py`는 host가 관리하는 기대 JSON과 실제 파일을 비교해. object key 순서는 무시하고, JSON type과 숫자의 파싱 결과 표현은 구별해. 예를 들어 true와 1, 1과 1.0은 서로 달라. duplicate key·nonfinite number·잘못된 JSON·없는 결과·읽을 수 없는 결과는 통과시키지 않아. CLI 입력은 파일당 1 MiB 이하여야 해.

```bash
python3 scripts/check-json-result.py \
  --expected /trusted/reference.json --actual result.json
```

기대값 파일과 검증 프로그램은 모델 workspace 바깥의 host 관리 경로에 두고, 기존 executable catalog와 completion-contract에 이 exact argv를 등록하면 돼. 새로운 xgen 설정 옵션은 추가하지 않았어. domain 규칙이나 정답 계산은 host checker에 두고 이 프로그램은 일반 JSON 비교만 해.

통과하면 exit code 0, 실패하면 1이야. stderr에는 다음 형태를 반환해.

```json
{"format_version":1,"status":"failed","reason":"value_mismatch","expected":{"total":5},"actual":{"total":4}}
```

이것은 설명용 예시야. JSON 진단은 줄바꿈 포함 4,096 bytes 이내이며 큰 값은 전체를 생략하고 `values_omitted:true`로 표시해. 잘린 JSON 조각은 내보내지 않아. control character와 Unicode는 JSON escape로 표현해. 누락·invalid JSON 오류는 결과 내용을 넣지 않고 reason만 반환해. 이 bound와 별도로 기존 process capture/context 한도가 적용돼.

다른 host checker에서는 `verify(expected, actual_path)`로 직접 호출할 수 있어. 테스트와 비교용 `feedback=False`는 같은 비교·exit code를 유지하면서 stderr만 비워. 기대값은 checker가 계산하거나 host가 관리하는 reference에서 읽어야 해.

**기대값과 실제값이 실패 진단으로 모델에 공개돼.** 숨은 oracle을 모델에 전달한 것은 아니지만, checker가 계산한 기대값으로 수정하는 효과를 측정하므로 독립적인 계산·추론 능력이 좋아졌다고 해석하지 않아. 비공개 내용을 진단에 포함할지는 host checker가 결정해야 해.

## 검증 경로

실제 Rust driver와 loopback model을 사용하는 numeric·Unicode 두 작업에서 다음 경로를 확인했어.

1. 틀린 파일 write → verify nonzero Receipt.
2. 해당 Receipt의 stderr 진단을 다음 model 요청의 toolOutputs에서 확인.
3. 현재 파일 read의 digest로 fs/write-atomic 수정.
4. 다시 verify exit code 0 → 최신 변경 이후 check Receipt 확인 → v2 완료 응답.

동일 checker의 stderr를 비운 대조군은 틀린 산출물로 완료를 제안했을 때 gate가 거절해. 이는 전달·수정·재검증 계약 검사이며 실제 모델 품질 증거는 아래 비교에서 따로 판단해.

## 실제 모델 사전 등록

새 numeric suffix totals·stable integer counts, Unicode UTF-8 hex·codepoint reverse 4개 입력을 사용해. 각 family의 design·validation 입력은 서로 다르고 이전 실험 입력을 재사용하지 않아. 처음부터 잘못된 result.json이 존재하므로 양쪽 목표에 현재 digest를 읽고 수정하라는 같은 계약을 넣었어.

F0와 F1은 동일한 completion gate, 목표, 도구, response schema, 모델 설정과 예산을 사용해. 검증자의 계산·판정은 같고 **stderr 진단 유무만 달라**. F0는 빈 stderr, F1은 bounded JSON 진단을 반환해. provider prompt와 request profile 설정도 양쪽 동일해. 검증 프로그램 파일 digest는 진단 flag 차이 때문에 달라.

`deepseek-flash`, `json_object`, thinking disabled, 최대 12 model turns에서 4개 입력 × 2회 반복 × 2개 조건 = 16회를 등록했어. case/repeat별 paired 순서를 균형 있게 섞고 fixture·binary·source·helper·interpreter·일정·모델 identity·quote를 고정했어. 반복은 독립적인 새 입력으로 세지 않아. 실패 이후 fixture 변경이나 trial 재시도는 하지 않아. 후보 거절 뒤 강제 repair나 자동 재개도 추가하지 않아.

- 입력·등록: `evals/completion-feedback/`
- 공개 결과: `evals/completion-feedback/results/2026-10-06/`
- 전체 원본 증거: `~/.local/share/xgen/evaluations/completion-feedback-2026-10-06`

4개 checker와 독립 oracle은 잘못된 기존 결과를 거절하고 올바른 참조 결과를 받아들이는 control을 먼저 통과했어. F1 진단의 expected 값도 참조와 일치했어. oracle 코드와 source는 모델 workspace에 두지 않아.

| 지표 | F0 진단 없음 | F1 JSON 진단 |
|---|---:|---:|
| 실행 | 8 | 8 |
| 실제 성공·수락된 완료 | 7 | 6 |
| 틀린 완료 | 0 | 0 |
| 검증 실패→성공 + 올바른 산출물 | 7 | 6 |
| 모델 호출 | 57 | 48 |
| quote 비용 USD | 0.017151408 | 0.013102038 |

design: F0 성공 3/4, F1 성공 4/4. validation: F0 성공 4/4, F1 성공 2/4. 

**진단 전달과 수정 경로는 동작하지만 전체 성공률 개선은 입증하지 못했어.** F0 7/8, F1 6/8이므로 기본 동작에 자동 적용하지 않아. helper는 host checker에서 선택적으로 쓸 수 있어. 이전의 0/8 성공과 비교해 개선됐다고 주장하지 않아. 작업과 초기 출력 상태·turn 한도가 달라서 각 단계의 수치를 직접 비교할 수 없어.

F1의 두 실패는 같은 validation-codepoint-reverse 입력의 반복이야. 첫 write가 결과를 변경한 뒤 verify가 실패했고, 두 번째 write에는 **최초 read의 오래된 digest**를 재사용해서 `EffectFailed`로 중단했어. 수정하려던 두 번째 내용은 기대값과 일치했지만 적용되지 않았어. 실패 recipe와 원본 tool outputs로 확인했고 실패를 성공률에서 제외하지 않았어. 반복 두 개를 독립 입력 두 개로 세지 않아.

다음 범용 문제는 수정 후 바뀐 파일의 최신 관측을 다음 write에 연결하는 거야. 현재 digest 비교를 우회하거나 강제 덮어쓰지 말고, 최신 digest를 재관측·참조하는 계약을 별도 새 사례에서 검증해야 해. 거절 뒤 무조건 retry를 추가하는 방식은 채택하지 않아.

이번 16회 비용은 $0.030253446, 직전 40회를 포함한 56회 총 비용은 $0.156258324야. 미정산 요청 0, 모델 identity 고정, 전체 일정·source·binary·입력·승인/Receipt 관계·비용 재계산 감사가 통과했어.

Python 78개 검사, 실제 bridge build, `cargo clippy --workspace --all-targets -- -D warnings`, fmt, public docs 및 diff 검사 통과. production Rust 엔진·provider prompt·기존 resume 계약은 변경하지 않았어.


## 재현과 한계

```bash
python3 scripts/analyze-completion-gate.py \
  --results ~/.local/share/xgen/evaluations/completion-feedback-2026-10-06 \
  --preregistration evals/completion-feedback/preregistration-2026-10-06.json \
  --output /tmp/completion-feedback-analysis.json
```

입력 보존·쓰기 범위·독립 oracle 성공, durable 완료, 실패→성공 check Receipt, model calls와 모든 trial의 비용을 따로 집계해. 승인/material/output/Receipt 관계와 실패 effect의 sidecar coverage를 검사해. 독립적인 cryptographic 재계산이나 OS sandbox 증명은 하지 않아. Python 실행 권한이 OS 파일 접근을 격리해 준다는 주장은 하지 않아.

가격은 [DeepSeek 공식 가격표](https://api-docs.deepseek.com/quick_start/pricing/)의 해당 UTC off-peak quote와 보존한 usage로 재계산해. 공급자 청구서와 대사한 금액은 아니야. 직전 40회 비용 $0.126004878을 포함해서 총 $0.50 이내로 제한했어.

## 현재 digest 후속 작업

[현재 digest 비교](current-digest-repair-2026-10-06.md)에서 같은 bounded raw read의 digest를 mismatch 진단에 추가했어. 새 paired 16회에서 digest 없음 4/8, digest 있음 8/8 성공을 관측했어. 직전 단계와 성공률을 합치지 않으며, 작은 표본의 결과로만 해석해.
