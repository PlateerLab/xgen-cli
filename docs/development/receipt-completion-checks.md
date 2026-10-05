# Receipt 기반 완료 검증 조건

2026-10-06. 범용 변경. `xgen run --completion-contract FILE`로 응답 schema와 필수 검증 명령을 한 파일에 지정해. Host가 해당 명령의 최신 Receipt와 실행 이후의 변경 여부를 확인한 뒤 완료 후보를 받아들여.

## 사용

```json
{
  "response_schema": {
    "type": "object",
    "properties": {"answer": {"type": "string", "minLength": 1}},
    "required": ["answer"],
    "additionalProperties": false
  },
  "checks": [{"id": "result", "argv": ["verify-result"]}]
}
```

`verify-result`는 작업 결과를 검사하고 통과하면 0, 실패하면 nonzero로 종료하는 host 관리 프로그램이야. 작업별 정답 규칙은 이 프로그램에 둬. 프로그램과 의존성은 신뢰할 수 있는 것으로 관리하고, 모델이 수정하는 workspace 바깥에 두는 방식을 권장해. Executable catalog는 지정한 프로그램의 경로와 내용을 기존 execution profile에 묶어.

```bash
xgen run "입력을 변환하고 결과를 검증해줘" \
  --completion-contract completion.json \
  --allow-dir . --allow-read --allow-write \
  --allow-executable verify-result=/absolute/path/to/verify-result \
  --allow-execute --allow-remote-model-egress
```

계산에 별도의 프로그램이 필요하면 기존 executable catalog에 그 프로그램도 선언해. 검증 조건을 설정하는 것이 도구 권한을 추가하지는 않아. `--allow-execute`를 생략하면 기존 승인 대기를 거쳐. `--response-schema`와 `--completion-contract`는 동시에 지정할 수 없어.

계약 파일은 32 KiB 이하이고, check는 1~8개야. 각 check의 `id`와 전체 `argv`는 서로 달라야 해. `argv[0]`은 선언된 executable logical ID여야 해. 명령은 shell 문자열 대신 argv 배열로 지정해. Check의 cwd는 workspace root `.`이고 invocation env override는 빈 object여야 해. 기존 host process environment는 그대로 사용해.

## 완료 조건

모델이 검증 명령을 일반 process Step으로 계획하고, 기존 Core·승인·adapter·verifier 경로에서 실행해. Host가 완료 순간에 검증을 별도로 실행하거나 숨겨서 재시도하지 않아.

1. 모든 check에 exact argv, cwd `.`, 빈 env override가 일치하는 검증된 process Receipt가 있어야 해.
2. 각 check의 가장 최근 실행이 정상 종료했고 exit code가 0이어야 해. 과거의 성공으로 최근 실패를 덮을 수 없어.
3. 검증 시작은 다른 잠재적 변경 effect의 Receipt 검증 완료보다 나중이어야 해. 파일 write, 다른 process, PTY 등 `ReadOnly`가 아닌 effect를 보수적으로 변경 경계로 취급해. 단순한 추가 file read는 기존 검증을 무효화하지 않아.
4. 여러 check는 같은 마지막 변경 이후에 모두 통과해야 해. Host가 지정한 exact check 실행끼리는 변경 경계에서 제외해. 검증 프로그램의 동작을 신뢰한다는 전제야.

Run의 전체 검증된 journal·Receipt·material recipe·typed output을 사용해. 모델이 적은 성공 문장이나 bounded planning context의 일부 기록을 통과 증거로 사용하지 않아.

검증 누락·실패·낡은 증거는 각각 `XGEN_COMPLETION_CHECK reason=missing|failed|stale`로 진단하고, 완료 후보를 `planner_invalid_response`로 거절해. stdout에 완료 응답을 출력하거나 completion sidecar를 commit하지 않아. 소비한 모델 호출은 기존 settlement와 usage 기록에 남아. 실패를 관찰한 모델이 이후 별도 turn에서 수정·재검증한 다음 완료를 제안하는 것은 가능해.

## 응답과 재개

새 조건을 선택한 Run은 `receipt-execution-report/v2`를 저장해. 기존 `commands`와 `response`에 다음 필드가 추가돼.

```json
{
  "format_version": 2,
  "response": {"answer": "검증 조건을 통과한 작업 결과"},
  "commands": [{
    "argv": ["verify-result"],
    "exit_code": 0,
    "execution_sequence": 12,
    "receipt_id": "receipt-example",
    "step_id": "step-example"
  }],
  "verification": {
    "status": "passed",
    "checks": [
      {
        "id": "result",
        "status": "passed",
        "argv": ["verify-result"],
        "exit_code": 0,
        "execution_sequence": 12,
        "receipt_id": "receipt-example",
        "step_id": "step-example"
      }
    ]
  }
}
```

위 값은 설명용 예시야. 실제 `commands`에는 검증 명령을 포함한 process 실행 이력이 들어가고, `verification.checks`는 그중 해당 check의 최신 성공 Receipt를 가리켜. 전체 JSON의 기존 5,000-byte 한도도 유지해.

Check 목록을 manifest에 저장하고 request profile digest에 묶어. 승인 후 재개할 때 원본 계약 파일은 필요하지 않아. 원래 workspace·catalog·model·권한·남은 예산을 사용하는 resume 계약은 유지해. 완료한 Run은 모델·도구 호출 없이 같은 응답을 재현해. Check가 없는 기존 Run은 기존 기본 출력이나 `receipt-execution-report/v1`을 유지해.

## 검증 자료와 한계

새 numeric artifact와 Unicode text artifact 작업을 사용해. 각 family에서 design 입력과 다른 held-out 입력을 하나씩 두고, 총 4개 입력에 8개 시나리오를 실행했어.

| 시나리오 | 완료 판정 |
|---|---|
| 올바른 결과 → 검증 성공 | 수락 |
| 올바른 결과, 검증 누락 | 거절: missing |
| 잘못된 결과 → 검증 실패 | 거절: failed |
| 검증 성공 → 결과 변경 | 거절: stale |
| 예전 성공 → 변경 → 최근 검증 실패 | 거절: failed |
| 검증 실패 → 수정 → 재검증 성공 | 수락 |
| 필수 검증 두 개 모두 성공 | 수락 |
| 두 번째 필수 검증 누락 | 거절: missing |

총 32개 시나리오에서 예상 판정을 확인했어. 후보 응답은 loopback model server가 반환하고, 프로그램은 실제 프로세스로 실행했어. 독립 checker가 input과 artifact를 직접 비교해. 수락 12회에는 Receipt 연결과 offline replay를 확인하고, 거절 20회에는 완료 sidecar가 저장되지 않은 것을 SQLite로 확인했어. Held-out Unicode 입력의 승인 대기 후 재개에서는 원본 계약 파일을 삭제해도 저장된 조건이 유지됐어.

이는 명령 기반 완료 gate의 결정적 통합 검증이야. 새 실제 모델의 정확도 개선 실험은 아직 수행하지 않았고, 이전 [24회 실험](final-response-contract.md)의 14/24 결과를 개선했다고 주장하지 않아. 검증 프로그램의 exit 0은 host가 정한 기준의 통과를 뜻해. 기준의 충분성, 임의의 자연어 답변의 참, 외부 파일의 이후 변경, OS sandbox를 증명하지 않아.

검증 프로그램이 자신의 입력이나 다른 check의 결과를 변경하거나, 모델이 수정한 코드만으로 스스로 통과를 선언하면 기준이 약해질 수 있어. 프로그램·의존성과 검사 대상의 범위는 host가 정해야 해. 엔진에 작업별 정답이나 데이터셋 이름을 넣지 않았어.

Rust workspace 669개 PASS, 기존 live 검사 4개 ignored. Python 71개 PASS에는 새 빌드의 기존 mock bridge 검증과 private journal mutation 검사를 포함해. 전체 workspace clippy, fmt, 공개 문서 계약 검사도 PASS야. Python 검사에서는 종료된 프로세스의 `/proc` read가 `ESRCH`를 반환하는 경합을 한 번 관측했어. 기존 runner와 같이 테스트도 `FileNotFoundError`와 `ProcessLookupError`를 모두 이미 사라진 프로세스로 처리한 뒤 전체 71개를 통과했어. 실행·정리 코드는 바꾸지 않았어.

## 실제 모델 후속 비교

[2026-10-06 실제 모델 비교](completion-gate-live-pilot-2026-10-06.md)에서 최초 24회와 다른 입력의 후속 16회를 보존했어. 후속 단계에서 틀린 완료 차단은 관측했지만 실제 작업 성공·수정 개선은 입증하지 못했어. 선택 옵션을 유지하고 기본 적용은 보류해.
