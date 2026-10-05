# 최종 응답 계약과 Receipt 기반 실행 기록

2026-10-05. 범용 변경. `xgen run --response-schema FILE`을 선택하면 모델의 최종 응답을 로컬 JSON Schema로 검증하고, host가 검증된 실행 기록을 붙여서 저장해. 기존 실행과 REPL의 기본 형식은 유지해.

## 사용

기존에 쓰던 model profile과 실행 권한을 그대로 사용해. 예를 들어 `response-schema.json`을 이렇게 작성해.

```json
{
  "type": "object",
  "properties": {"answer": {"type": "string", "minLength": 1}},
  "required": ["answer"],
  "additionalProperties": false
}
```

```bash
xgen run "README 내용을 요약해줘" \
  --allow-file README.md --allow-read --allow-remote-model-egress \
  --response-schema response-schema.json
```

Schema는 `response` 내부 객체의 계약이야. 최종 응답의 바깥 형태는 host가 고정해.

```json
{
  "format_version": 1,
  "response": {"answer": "검증한 작업 결과 설명"},
  "commands": [
    {
      "argv": ["python3", "-B", "test_visible.py"],
      "exit_code": 1,
      "execution_sequence": 12,
      "receipt_id": "receipt-example",
      "step_id": "step-example"
    }
  ]
}
```

위 ID와 sequence는 설명용 예시야. 파일 읽기만 실행했다면 `commands`는 빈 배열이야. `argv[0]`은 승인된 executable catalog의 logical ID야. `execution_sequence`는 전체 journal의 실행 시작 sequence라서 연속 번호일 필요가 없어. 실제 stdout의 JSON은 canonical 형식으로 저장·출력해.

## 검증과 저장 경계

모델은 `summary` 문자열 안에 schema에 맞는 객체만 반환해. Provider는 중복 key, 깊이 제한, JSON 형식과 schema를 로컬에서 검사해. `json_schema`와 `json_object` 모두 이 검사를 거쳐. 잘못된 후보는 Core에 completion으로 commit하지 않고, 소비한 모델 호출은 기존 usage settlement에 남겨. 자동 재시도는 추가하지 않았어.

그다음 CLI composition의 `ReceiptReportPlanner`가 전체 Run의 검증된 journal과 Receipt를 다시 읽어. 현재 model call reservation과 planning context의 head도 대조해. 제한된 planning context에 과거 실행이 빠져 있어도 전체 기록을 사용해.

| 최종 필드 | 근거 |
|---|---|
| `argv` | Receipt의 input digest와 Step에 연결된 검증된 invocation/material recipe |
| `exit_code` | Receipt의 output digest와 일치하는 typed tool output의 `exitCode` |
| 배열 순서·`execution_sequence` | 해당 Receipt의 effect에 연결된 `EffectExecutionStarted` journal sequence |
| `receipt_id`, `step_id` | 검증된 execution Receipt |

Proposal의 Step 순서, Receipt가 끝난 순서, 모델이 적은 명령 목록을 실행 순서로 사용하지 않아. nonzero exit도 실제 값 그대로 기록해. Recipe·output·start의 연결이 없거나 변조됐으면 최종 후보를 거절해.

완성한 envelope를 기존 Core의 summary 제한에 맞춰 검사한 뒤 durable completion에 commit해. 전체 UTF-8 JSON은 기존 한도인 5,000 bytes 이내여야 해. 넘으면 실패하며 명령을 잘라내거나 요약해서 기록을 누락하지 않아. 긴 실행의 별도 report sidecar는 후속 작업이야.

## 재개와 호환성

Schema는 Run manifest에 저장하고 request profile digest에도 묶어. 승인 대기 후 `xgen resume RUN_ID`로 재개할 때 원본 schema 파일을 다시 읽을 필요가 없어. 기존 workspace, catalog, 권한과 model 설정은 기존 resume 계약을 따라야 해.

완료된 Run의 resume은 검증된 durable completion을 그대로 재현해. 추가 모델 호출이나 명령 실행 없이 같은 응답을 보여줘. Schema가 없는 기존 Run의 manifest serialization과 request profile digest는 기존 형식을 유지해. Rust embedding은 `LocalRunRequest.final_response_schema`로 같은 기능을 선택할 수 있어.

Schema는 32,768 UTF-8 bytes 이하, JSON 깊이 32 이하의 로컬 object schema만 허용해. 기존 artifact schema의 제한된 vocabulary를 재사용하고 `$ref`와 외부 resource loading은 허용하지 않아. 새로운 schema engine이나 도메인별 규칙을 추가하지 않았어.

## 보장 범위

이번 `commands`는 `xgeny.process/execute` 실행만 포함해. PTY session operation, web search, 파일 operation을 일반 process 명령으로 바꾸지 않아. cwd·환경변수·stdout 전체를 보고하는 계약도 아니야.

Schema 준수는 `response`에 적힌 설명의 의미적 정확성이나 작업 성공을 증명하지 않아. 작업 결과는 독립 task oracle로 검증해야 해. 프로세스 승인과 Receipt의 연결을 검증하는 것도 OS sandbox의 증명과는 구분해.

## 검증 자료

서로 다른 두 schema와 두 provider dialect에서 형식·필수 필드·타입·중복 key 위반을 거절하는 검사를 추가했어. Process 통합 검사는 실제 exit 7과 0, proposal과 다른 실행 순서, 승인 후 재개, schema 원본 삭제 후 재개, 완료 응답의 offline 재현을 확인해. File-only 검사에서는 `commands=[]`와 잘못된 후보의 미저장을 확인해. Material 검사는 두 다른 argv에 대해 Step·digest 불일치와 recipe 변조·삭제를 거절해.

Python mock model 검사는 file/process가 섞인 실행을 포함해 같은 envelope를 독립 SQLite 조회 결과와 대조해. 이전 [120회 pilot](planning-model-pilot-results-2026-10-05.md)의 형식·순서 실패에서 일반적인 경계를 잡되, 새 실제 모델 검증은 별도의 [사전 등록](../../evals/final-response-contract/preregistration-2026-10-05.json)과 [새 fixture](../../evals/final-response-contract/cases.json)를 사용해.

Artifact 변환과 Python CLI 수정 두 family에서 design 2개와 held-out validation 2개를 고정했어. 각 작업을 X0/X1/XN 조건별 2회 실행해 총 24회야. 모든 조건에 새 계약을 적용하므로 이전 94/120과 정확도 개선률을 비교하지 않아. 두 반복 실행을 독립 작업 두 개로 세지 않아.

## 실제 모델 결과

[전체 원자료](../../evals/final-response-contract/results/2026-10-05/analysis.json)와 [summary](../../evals/final-response-contract/results/2026-10-05/summary.json)를 보존했어. 실행 중 source·binary·fixture 변경과 자동 재시도는 없었어.

| 항목 | 결과 |
|---|---:|
| 등록 실행 완료 | 24/24 |
| 응답 형식·실행 evidence 일치 | 24/24 |
| 작업 oracle·수정 범위·전체 acceptance | 14/24 |
| 형식·실행 기록의 잘못된 주장 | 0 |
| 잘못된 작업 완료 주장 | 10 |
| 사전 승인·material·Receipt 관계 audit | 24/24, 72 effects |
| 중복 effect 시작·불확정 사용량 | 0 |
| 모델 HTTP 요청 | 88, 전부 200 |
| Usage × 등록 단가 | $0.025009278 |

X0와 XN은 각각 4/8, X1은 6/8 accepted야. Held-out만 보면 X0·XN은 각각 2/4, X1은 4/4야. Model response identity는 등록한 `deepseek-flash`와 fingerprint를 유지했어. 가격은 실행 전후 [DeepSeek 공식 단가](https://api-docs.deepseek.com/quick_start/pricing/)의 snapshot hash가 같았고, 위 금액은 보존한 usage로 재계산했어. Provider 청구서와 대조한 금액은 아니야.

실패 10회는 다른 두 artifact 작업에서 나왔어. Text metadata는 예상 `nonempty_lines=2`, `utf8_bytes=34`인데 byte 수를 35 또는 38로 적었고, 한 번은 line 수도 3으로 적었어. Base64 chunk 변환은 hex가 맞았지만 실제 8 bytes를 7이라고 적었어. Python CLI 수정은 12/12 통과했어.

두 artifact 작업은 filesystem 도구만 선언했어. Model이 계산한 숫자를 외부 계산 도구로 확인하는 과정이 없었으므로, JSON Schema와 Receipt 연결만으로 이 오류를 막지 못했어. 이번 실험은 artifact 계산에 어떤 도구 구성이 필요한지나 새 계약의 인과적 효과를 분리하는 실험이 아니야.

등록한 전체 gate는 **FAIL**이야. 구조적 계약 관련 검사는 통과했지만 `all_tasks_accepted`와 `no_false_completion`은 실패했어. 이 결과로 기본 출력이나 planning 크기를 변경하지 않아. 이번 validation 두 사례도 관찰한 데이터가 됐으므로 후속 개선의 새 held-out 사례로 재사용하지 않아.

Rust workspace 665개 PASS, 4개 기존 live 검사 ignored. Python 71개 PASS에는 mock bridge 실행과 private development journal mutation 검사를 포함해. `cargo fmt`, workspace 전체 `clippy -D warnings`, 공개 문서 계약 검사도 PASS야. 독립 Python audit는 journal·sidecar의 관계와 usage 단가를 대조했으며 모든 signature/digest의 독립 재계산이나 OS sandbox 증명은 수행하지 않았어.

SQLite와 binary는 Git에 넣지 않고 `~/.local/share/xgen/evaluations/final-response-contract-2026-10-05`에 보존했어. 공개 metadata와 private 사본의 hash를 대조했고, 실행 시점 runner·analysis·audit helper snapshot을 공개 결과에 함께 보존했어. 분석 재현은 같은 source checkout에서 다음과 같이 실행해.

```bash
python3 evals/final-response-contract/results/2026-10-05/analysis-at-execution.py \
  --repository "$PWD" \
  --results "$HOME/.local/share/xgen/evaluations/final-response-contract-2026-10-05" \
  --registration evals/final-response-contract/preregistration-2026-10-05.json
```

분석 시 source·binary·등록 schedule·config·fixture hash도 대조해. 다른 checkout이나 소스 변경 후에는 기존 실험의 source 일치 검사가 실패할 수 있어.
