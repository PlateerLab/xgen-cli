# ADR-0057: 검증된 기록으로 만든 변경 내역 표시

- 상태: Accepted
- 기준일: 2026-10-26
- 변경 분류: 범용 표시 계약. 특정 도구·파일·업무 규칙 없음.
- 기존 경계: ADR-0033(REPL durable progress), ADR-0038, ADR-0054
- protocol·SQLite schema·journal 변경: 없음. 이미 저장된 기록을 읽기만 한다.

## 문제

REPL은 Run이 끝나면 모델의 최종 요약과 `· 12s · 6 committed`만 보여 줬다. 어떤 파일이 어떻게
바뀌었는지는 볼 수 없었다. ADR-0033이 progress event에서 path와 파일 내용을 뺐고, 쓰기 승인도
경계 단위라 무엇을 쓸지 보여 주지 않는다.

그래서 모델 요약이 틀려도 사용자가 알 수 없었다. 재현 실행(main 3111d74, deepseek, 3개 저장소 x
3회)에서 한 저장소의 요청은 3회 모두 README에 이미 있는 행을 중복으로 추가했다. 그중 2회는 요약이
"누락된 항목이 없다"고 말하면서 patch를 했다고 적었다. 사용자에게 보이는 것은 그 요약뿐이었다.
요약이 실제로 하지 않은 수정을 했다고 말한 경우는 9회 중 0회였다. 문제는 요약의 거짓말이 아니라
요약 외에 확인할 수단이 없다는 점이다.

## 결정

### 1. 변경 내역은 검증된 journal 기록에서만 만든다

Run이 끝난 뒤 `inspect_local_changes(run_id)`가 Run store를 읽기 전용으로 열어 다음을 Receipt 순서로
모은다.

- Receipt 상태가 `Succeeded`이고 Step이 `Completed`인 것만 쓴다. 실패하거나 불확정인 Step은 없다.
- 인자는 사용자가 승인한 그 Step의 material recipe에서 가져온다. recipe의 digest, Step, material
  digest, capability가 journal의 material binding과 맞지 않으면 무결성 오류다.
- `changed` 여부는 verifier가 확인한 tool output에서 가져온다.
- workspace를 다시 읽지 않는다. 모델 요약도 쓰지 않는다.

대상 capability와 표시 내용:

| capability | 표시 |
| --- | --- |
| `xgeny.fs/apply-patch` | path, 추가·삭제 줄 수, edit마다 양쪽이 공유하는 앞뒤 줄을 뺀 `-`/`+` 줄 |
| `xgeny.fs/write-atomic` | path, 생성 또는 교체, 줄 수. 생성이면 내용 줄 |
| `xgeny.process/execute` | 실행 파일의 logical id와 인자. 파일 변경은 추적하지 않는다고 적는다 |
| `xgen.terminal/start`, `xgen.terminal/write` | 터미널 입력이 있었다는 사실. 파일 변경은 추적하지 않는다고 적는다 |

읽기 전용 capability와 web search는 workspace를 바꾸지 않으므로 넣지 않는다.

### 2. REPL 표시

- 이번 진행(leg)에서 commit된 Step이 있을 때만 읽는다. 답변 뒤, footer 앞에 `Changes` 블록을 쓴다.
  완료뿐 아니라 거절(`failed_work` 등)과 일시 정지로 끝나도 그때까지의 변경을 보인다.
- 같은 Run을 `/resume`하면 이전에 보인 항목은 다시 쓰지 않는다.
- 변경 하나당 12줄, 전체 40줄까지만 쓰고 나머지는 `… N more lines`로 줄인다. 명령은 120자까지만 쓴다.
- path, 내용, 인자의 모든 control character(줄바꿈 포함)를 escape한다. terminal 제어가 불가능하다.
- 기록을 읽지 못하면 `Changes could not be read from the Run journal.`만 쓰고 REPL은 계속한다.

### 3. `xgen run` 표시

stderr에 변경마다 한 줄을 쓴다. 문자열 값은 JSON 문자열이라 줄을 깨지 못한다.

```text
XGEN_CHANGE run_id=<id> kind=patch path="src/lib.rs" changed=true added=1 removed=1
XGEN_CHANGE run_id=<id> kind=write path="notes.md" changed=true created=true lines=12
XGEN_CHANGE run_id=<id> kind=execute executable="cargo"
XGEN_CHANGE run_id=<id> kind=terminal
```

파일 내용과 명령 인자는 쓰지 않는다. 기록을 읽지 못하면 `XGEN_CHANGES_UNAVAILABLE run_id=<id>`를 쓴다.

### 4. ADR-0033과의 관계

ADR-0033의 progress event는 그대로 path와 내용을 싣지 않는다. 이 결정은 실행 중 스트리밍이 아니라
끝난 뒤의 표시다. 사용자가 이미 승인한 인자와 verifier가 통과시킨 결과만 보이므로 검증 전 정보가
나가지 않는다.

## 대안

- **요약과 Receipt를 자동 대조:** 자연어 요약을 해석하는 판정이 필요하고 모델 품질에 따라
  달라진다. 사실 기록을 보여 주는 쪽이 일반적이다.
- **쓰기 승인 전 diff 표시:** 사용자가 쓰기를 미리 고를 수 있지만 승인 단위를 경계에서 Step으로
  바꾸는 별도 설계가 필요하다. 이번 범위에 넣지 않았다.
- **workspace를 다시 읽어 diff:** 다른 편집과 섞이고, Run이 한 일과 구분할 수 없다.

## 검증

- 단위: edit에서 공유하는 앞뒤 줄을 빼는 규칙, canonical path와 executable id 표시, 줄·명령 상한,
  control character escape, `/resume` 뒤 새 항목만 표시, commit이 없으면 읽지 않음.
- e2e: stale digest patch가 거절되고 바른 patch가 적용된 Run에서 `XGEN_CHANGE`는 적용된 patch 한
  줄뿐이다.
- 실사용: 대화형 `xgen`, deepseek profile. 재현에 쓴 3개 사례를 사례마다 3회 실행했다.

  | 사례 | 화면의 변경 내역 | git diff |
  | --- | --- | --- |
  | ku-portal-mcp, 도구 목록 비교 (설계) | 3회 모두 `README.md patched +1 -0`과 중복 행 내용 | 3회 모두 일치 |
  | graph-tool-call, timeout 문서화 (설계) | `+1 -1`, `+2 -1`, `+1 -1` | 3회 모두 일치 |
  | gwanjong-mcp, 정책 표 (검증용) | `+18 -0`, `+18 -0`, `+30 -0`, 앞 12줄과 나머지 줄 수 | 3회 모두 일치 |

  추가 확인:
  - edit 3개짜리 patch는 `+3 -3`과 바뀐 세 줄만 보였다.
  - 9 MB 파일의 마지막 줄 수정(최종 binary, 3회): 2회는 모델이 승인된 python3 실행으로 고쳤다.
    실행한 명령 2~3개가 줄바꿈이 escape되고 120자로 잘린 채 `(file changes not tracked)`와 함께
    보였고, git diff는 `+1 -1`이었다. 1회는 commit 전에 거절돼 `Changes` 블록이 없었다.
  - 첫 번째 사례에서 중복 행이 이제 화면에 그대로 보인다. 요약이 "누락 없음"이라고 해도 사용자가
    잘못된 수정을 알아챌 수 있다.
