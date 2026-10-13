# 검증 결과의 현재 digest로 수정하기

2026-10-06. 범용 변경. JSON 검증 진단에 `actual_digest`를 추가해 모델이 가장 최근에 검증한 결과 파일의 digest를 다음 `fs/write-atomic.expectedDigest`에 사용할 수 있게 했어. 새 xgen 설정 옵션은 추가하지 않아.

## 원인과 변경

직전 [검증 진단 비교](completion-feedback-repair-2026-10-06.md)에서 모델은 실패 진단을 보고 두 번째 수정할 내용은 맞게 만들었어. 그러나 첫 write가 파일을 변경한 뒤에도 최초 read의 digest를 재사용했고, 수정은 적용되지 않았어. 이 두 실패는 같은 Unicode 입력의 반복이며 독립 입력 두 개로 세지 않아.

이번 변경 전에 숫자·Unicode의 다른 작업에서 write → 검증 실패 → 다음 write를 재현하는 실제 driver 계약 검사를 구성했어. 별도 현재 read가 있으면 수정이 가능하고, 관측 뒤 파일이 변경되면 이전 digest의 write를 거절하는 기존 경계는 유지해야 해.

`scripts/check-json-result.py`의 `read_json_snapshot`은 bounded raw bytes를 한 번 읽고, **같은 bytes에서 JSON parse와 SHA-256 계산**을 해. parse한 내용과 checksum이 서로 다른 읽기에서 나오지 않아. 잘못된 JSON 값에 대한 진단은 다음 형태야.

```json
{
  "format_version": 1,
  "status": "failed",
  "reason": "value_mismatch",
  "expected": {"total": 5},
  "actual": {"total": 4},
  "actual_digest": "sha256:<raw-result-bytes의 SHA-256>"
}
```

digest는 예시 자리표시자야. 실제 값은 lowercase hex 64자리이며 raw bytes의 공백·줄바꿈·UTF-8 표현을 포함해. JSON을 다시 serialize한 bytes의 checksum을 쓰지 않아. `verify`와 CLI는 기본으로 mismatch 진단에 digest를 포함해. 평가용 `include_digest=False`는 같은 판정·기대/실제 진단을 유지하고 digest 필드만 생략해.

기존 파일 읽기 1 MiB와 진단 줄바꿈 포함 4,096-byte 한도를 유지해. 큰 expected·actual 값은 전체를 생략하면서 digest는 남겨. control character는 JSON escape로 표현해. missing·invalid·unreadable 결과 오류에는 digest를 주지 않으므로 현재 파일을 다시 관측하는 기존 경로를 사용해야 해.

진단은 process Receipt의 비신뢰 데이터야. 이것이 실행 권한이 되거나 `expectedDigest`를 host가 몰래 바꾸는 것은 아니야. 모델이 기존 write argument에 그 값을 사용하고, 기존 승인·material·adapter·verifier 경로가 실행해. 진단 후 파일이 바뀌면 기존 digest 비교가 쓰기를 거절해. 내용의 의미적 정답은 신뢰할 수 있는 checker와 독립 oracle로 따로 판단해.

## 계약 검사

숫자 JSON·Unicode JSON 두 작업에서 동일한 원인·수정 경로를 검증했어.

- digest 없는 진단: 현재 file read → 그 read의 digest로 수정 → 재검증 → 완료, 6 model calls.
- digest 있는 진단: 추가 file read 없이 검증 snapshot digest로 수정 → 재검증 → 완료, 5 model calls.
- 검증 완료 뒤, 다음 write 실행 전에 외부 변경을 넣으면 snapshot digest의 write가 실패하고 외부 내용이 보존돼. 이 시점의 변경에 대한 회귀 검사이며 임의 OS writer와의 완전한 동시성 격리를 증명하지 않아.

별도 unit 검사에서 snapshot read가 끝난 직후 파일 내용을 바꿔도 진단의 actual과 digest가 원래의 같은 bytes를 가리키는지 확인했어. 외부 변경 후 새 내용을 재읽어 과거 actual에 붙이는 오류를 막아. 큰 Unicode 값의 진단 생략 뒤에도 digest가 한도 내에서 남는지 확인해.

## 실제 모델 사전 등록

새 numeric window extrema·integer run-length encoding, Unicode codepoint ordinal·ASCII case flip 4개 입력을 사용해. 각 family의 design·validation 입력은 서로 다르고 이전 실험 입력을 재사용하지 않아.

D0와 D1은 동일한 completion gate, 목표, 도구, response schema, provider prompt, 모델 설정·예산을 사용해. 양쪽 모두 rich expected·actual 진단을 줘. **D1에만 같은 raw read의 actual_digest를 추가해.** 양쪽 목표에는 최신 관측을 사용하고, digest 진단이 없으면 최신 write Receipt나 현재 file read를 사용하는 같은 규칙을 적었어. 따라서 공급한 정보가 추가됐을 때의 비교이며, 설명이 없는 자유형 작업에서의 성능을 주장하지 않아.

DeepSeek `deepseek-flash`, `json_object`, thinking disabled, 최대 12 model turns에서 4개 입력 × 2회 반복 × 2개 조건 = 16회를 고정했어. case/repeat별 paired 순서를 균형 있게 섞고, fixture·source·helper·binary·interpreter·일정·model identity·quote를 실행 전에 등록했어. 반복은 독립 입력으로 세지 않아. 실패 뒤 설정이나 fixture를 수정하거나 trial을 재시도하지 않아.

4개 checker·독립 oracle은 잘못된 기존 결과를 거절하고 올바른 참조 결과를 통과시키는 control을 확인했어. D0·D1 초기 진단은 digest 필드 외에는 같고, D1 digest가 raw result file과 일치해. checker가 계산한 기대값은 양쪽 모델에 공개되지만 숨은 oracle 코드는 workspace에 두지 않아. 모델의 독립 추론 능력이 좋아졌다고 해석하지 않아.

- 입력·등록: `evals/current-digest/`
- 공개 결과: `evals/current-digest/results/2026-10-06/`
- 전체 원본: `~/.local/share/xgen/evaluations/current-digest-2026-10-06`

| 지표 | D0 digest 없음 | D1 digest 있음 |
|---|---:|---:|
| 실행 | 8 | 8 |
| 실제 성공·수락된 완료 | 4 | 8 |
| 틀린 완료 | 0 | 0 |
| 검증 실패→성공 + 올바른 산출물 | 4 | 8 |
| 모델 호출 | 35 | 41 |
| quote 비용 USD | 0.008741124 | 0.010600776 |

design: D0 성공 2/4, D1 성공 4/4. validation: D0 성공 2/4, D1 성공 4/4. 

**이 비교에서는 digest 제공 조건의 실제 성공이 4/8→8/8로 늘었어.** D0의 ordinal·integer runs에서 기존 파일에 expectedDigest=null을 사용한 4회가 EffectFailed로 중단됐고, D1은 같은 case/repeat pair에서 최신 진단 digest로 write·verify·완료를 했어. 두 family에서 관측했지만 독립 입력은 총 4개이며, 모든 실제 작업의 일반 성공률 향상이나 통계적 유의성을 주장하지 않아. D0가 이 단계에서 오래된 digest를 재사용한 사례는 없었으므로 실제 모델의 stale-after-write 문제 자체가 해결됐다고 주장하지 않아. 그 경로와 외부 변경 보호는 별도의 실제 driver 회귀 검사로 확인했어.

D0의 저렴한 실패도 전체 비용에 포함해. 먼저 실패한 Run이 적게 호출했다는 이유로 효율이 좋다고 해석하지 않고, 성공률·전체 호출·전체 비용을 함께 보고해. helper의 mismatch 진단에는 현재 digest를 제공하되, production 엔진의 쓰기·승인·재개·거절 정책을 변경하거나 모든 작업에 verifier를 강제로 추가하지 않아.

이번 16회 quote 비용은 $0.019341900, 이전 56회를 포함한 72회 총 비용은 $0.175600224야. 미정산 요청 0, 전체 일정·source·binary·입력·model identity·승인/Receipt 관계·비용 재계산 감사 통과.

등록 초기 bytes와 성공 write recipe의 raw bytes를 시간 순서대로 추적한 독립 snapshot audit도 16회 모두 통과했어. 진단의 actual·actual_digest를 다시 계산하고, 성공 write의 precondition·output digest와 실패 write의 conflict evidence를 확인해. 추적하지 못하는 process나 mutation은 성공으로 인정하지 않아. 이는 기존 관계 binding 감사에 추가한 실행 후 검사이며 전체 journal crypto verification을 대신하지 않아. 숫자·Unicode journal 복사본의 actual_digest를 변조하면 감사가 실패하는 것도 확인했어.

Python 80개 전체 회귀와 별도 실제 journal 변조 검사 1개, 실제 bridge build, clippy all-targets, fmt, public docs·diff 검사 통과. 다음은 이 helper를 실제 프로젝트의 host verifier와 연결한 end-to-end 사례로 확인하는 단계야. CLI 기본 Run에 대한 자동 적용이나 배포는 이 실험 범위가 아니야.


## 재현과 한계

```bash
python3 scripts/analyze-completion-gate.py \
  --results ~/.local/share/xgen/evaluations/current-digest-2026-10-06 \
  --preregistration evals/current-digest/preregistration-2026-10-06.json \
  --output /tmp/current-digest-analysis.json
```

일정·source·binary·입력, 승인/material/output/Receipt 관계, 실패 effect의 sidecar coverage와 모든 trial 비용을 검사해. crypto digest/signature의 전체 독립 재계산이나 OS sandbox 증명은 하지 않아. 이전 실험 결과와 성공률을 합쳐 개선으로 보이지 않게 하고 새 paired 단계 자체를 보고해.

가격은 [DeepSeek 공식 가격표](https://api-docs.deepseek.com/quick_start/pricing/)의 해당 UTC off-peak quote와 보존한 usage로 재계산해. 이번 실행 전에 공식 snapshot hash가 등록값과 같은지 재확인했어. 공급자 청구서와 대사한 금액은 아니야. 직전 56회의 $0.156258324를 포함해서 총 $0.50 이내로 제한했어.
