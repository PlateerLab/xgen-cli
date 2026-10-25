# ADR-0055: 인자가 어긋난 확정 실패 뒤 계획 계속

- 상태: Accepted
- 기준일: 2026-10-26
- 변경 분류: 범용 runtime 계약. 특정 도구·파일·업무 규칙 없음.
- 기존 경계: ADR-0038, ADR-0052, ADR-0054
- protocol·SQLite schema 변경: 없음. journal 이벤트 형식도 그대로다. reducer의 완료 수락 조건만
  넓어진다.
- planner prompt 변경: 없음. planning context의 실패한 Step에 고정 문장 `failureMeaning`이 붙는다.

## 문제

ADR-0052 이후에도 read-only가 아닌 Step이 확정 실패하면 Run이 `failed_work`로 끝났다. 아무것도
바뀌지 않았고 다른 인자로 다시 요청하면 성공하는 경우도 마찬가지였다.

대표 사례는 에이전트가 파일을 읽은 뒤 사용자가 에디터로 같은 파일의 다른 줄을 고친 경우다.
apply-patch는 `expectedDigest`가 달라 `precondition_conflict`로 거절한다. 다시 읽고 패치하면 되는
상황인데 요청 전체가 끝난다. 사용자는 같은 요청을 처음부터 다시 해야 했다.

## 결정

### 1. 인자가 어긋난 확정 실패는 settled로 본다

`StepStatus::Failed`인 Step이 다음 중 하나이면 *settled failure*로 분류한다.

- durable intent가 `read_only`다 (ADR-0052, 기존).
- adapter가 보고한 failure code(ADR-0054)가 *인자 불일치*다. 요청의 인자가 현재 workspace와 맞지
  않아 거절된 실패다.

| 인자 불일치 (settled) | 상한·환경 (계속 멈춤) |
| --- | --- |
| `not_found`, `not_a_file`, `not_a_directory` | `too_large`, `not_utf8`, `limit_exceeded` |
| `precondition_conflict` | `unavailable`, `io_error` |
| `patch_context_missing`, `patch_context_ambiguous`, `patch_edits_overlap` | code 없음: 검증 실패, 재조정 실패 |

이 분류는 두 성질에 근거한다.

- **효과 없음**: adapter는 요청한 효과를 적용하기 전에 확정 실패한 경우에만 failure code를 붙여
  `Failed`를 보고한다. 효과가 적용됐을 수 있는 실패는 `Unknown`으로 보고한다. filesystem 쓰기
  adapter는 모든 확정 실패가 commit rename 이전에 일어난다. 검증·재조정 실패는 code가 없으므로
  settled가 되지 않는다.
- **고칠 수 있음**: 인자 불일치는 다른 인자로 다시 요청하면 성공할 수 있다. 상한과 환경 실패는
  같은 대상을 다시 요청해도 같은 결과가 난다. 그래서 settled로 두면 모델이 같은 요청을 반복한다.
  아래 검증의 첫 시도에서 실제로 그랬다.

frontier는 `read_only_failed_step_ids` 대신 `settled_failed_step_ids`를 낸다.
`has_blocking_failures`, `all_steps_settled`, `terminal_quiescence`와 `TaskCompletion` 수락은 ADR-0052와
같은 자리에서 이 집합을 쓴다. 실패한 Step은 계속 dependency를 막는다.

### 2. planning context에 code의 뜻을 고정 문장으로 싣는다

실패한 Step 요약에 `failureMeaning`을 붙인다. code마다 정해진 영어 문장 하나이고, 값(경로, 크기,
내용, OS 오류)은 넣지 않는다. 문장은 지시가 아니라 사실만 말한다. 예를 들어
`precondition_conflict`의 문장은 "대상이 관찰 뒤 바뀌어 이전 관찰은 낡았고, 아무것도 바뀌지 않았다"는
뜻이다. 그래서 planningContext를 신뢰하지 않는 데이터로 다루는 prompt 계약 안에 머문다.

code만 실었을 때 모델은 `precondition_conflict` 뒤에 같은 digest로 같은 패치를 23번 반복했다.
문장을 실은 뒤에는 다시 읽고 패치했다.

### 3. 사용자 표시

Run을 멈춘 Step의 label만 보인다. 이제 그 Step은 상한·환경 실패이거나, 검증 실패
(`verification_failed`)다. 출력 형식(`XGEN_STEP_FAILURE`, `A step failed:`)은 ADR-0054와 같다.

## 결과

- 동시 수정, 잘못 짚은 경로, 맞지 않는 patch 문맥은 요청을 끝내지 않는다. 모델이 다시 읽고 고칠 수
  있다.
- 반복 실패는 tool-call과 model turn budget으로 끝난다 (ADR-0015).
- 상한 실패(예: 64 KiB를 넘는 파일 수정)는 전과 같이 바로 멈추고 code를 보인다.
- 이 변경 이전 binary는 settled write failure 뒤에 수락된 완료가 있는 journal을 replay하지 못한다.
  기존 journal은 새 binary에서 그대로 replay된다.

## 검증

- workgraph: read-only 실패와 인자 불일치 code가 있는 쓰기 실패는 settled다. 상한 code나 code 없는
  쓰기 실패는 blocking이다.
- e2e:
  - stale digest patch가 실패한 뒤 다음 planning context에 `failureCode: precondition_conflict`와
    `failureMeaning`이 있다. 이어서 바른 digest의 patch가 적용되고 Run이 exit 0으로 끝난다.
    gate를 되돌리면 이 테스트가 `failed_work`로 실패한다.
  - 64 KiB를 넘는 파일의 patch는 `XGEN_STEP_FAILURE ... code=too_large`로 멈추고 파일은 그대로다.
- 실사용: 대화형 `xgen`, default profile, 이전 main(v0.1.0-rc.5)과 이 변경을 사례마다 3회씩 실행했다.
  쓰기 승인 프롬프트가 떴을 때 같은 파일의 다른 줄을 수정해 사용자의 동시 편집을 재현했다.

  | 사례 | 이전 | 이후 |
  | --- | --- | --- |
  | devhub-social, timeout 한 곳 수정 (설계) | 3회 모두 `failed_work`, 8~10초 | 3회 모두 완료, 4단계, 12초, 사용자 수정 보존 |
  | gwanjong-mcp, 기본값 세 곳 수정 (설계) | 3회 모두 `failed_work` | 3회 모두 완료, 4~5단계, 12~14초, 세 값 모두 정확 |
  | ku-portal-mcp, 같은 값 두 곳 수정 (검증용, 설계에 안 씀) | 3회 모두 `failed_work` | 3회 모두 완료, 4단계, 12초, 두 곳 모두 정확 |

  회귀 확인:
  - 113 KB 파일의 수정 요청: 첫 설계(모든 code settled)에서는 `too_large` 패치를 21번 반복하다
    턴 예산을 소진했다. 최종 설계에서는 3회 모두 10~12초에 `too_large`로 멈췄고, 이전 main과 같다.
  - 없는 CHANGELOG.md 요약(ADR-0054 사례): 3회 모두 답했다. 실패한 Step 수는 1, 1, 2였다.
  - 실사용에서 `patch_context_*` 실패는 자연스럽게 나오지 않았다. 이 code들의 계속 동작은 같은
    분류 규칙과 단위 테스트로만 확인했다.
