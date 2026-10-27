# ADR-0069: 나중에 바뀐 자원의 관찰 결과에 표시 달기

- 상태: Accepted
- 기준일: 2026-10-27
- 변경 분류: 범용 planning context. 특정 작업·언어·모델 규칙 없음.
- protocol·journal 변경: 없음. planning context v3의 `toolOutputs` 항목에 선택 필드 두 개가 붙는다.

## 문제

여러 파일을 고치는 rename 작업에서 모델이 이미 적용한 patch를 계속 다시 제안해 `patch_context_missing`으로
실패하고 예산을 다 썼다. 실패한 patch를 원본에 순서대로 다시 적용해 분류하면 H6(필드 rename)의
`patch_context_missing` 271개 전부가 "oldText는 없고 newText는 이미 있는" 재적용이었다. 13번 실행 중 8번이
재적용 15~55회를 냈다.

모델이 보는 기록을 보면, patch 전에 한 검색 결과(`devto.py:199 likes=...`)와 읽기 결과가 그대로 남아 있고 모델은
그 옛 줄을 oldText로 복사했다. 같은 파일을 patch 뒤에 다시 읽은 결과가 있어도 옛 결과를 썼다.

기록 재현(H6 실행 7개에서 재적용 직후 14개 지점, 지점당 3회, deepseek flash)으로 원인을 확인했다.

| 맥락 변경 | 재적용 제안 |
| --- | --- |
| 그대로 | 25/42 |
| 실패 이유 문장을 "이미 적용된 것 같다"로 바꿈 | 22/42 |
| 이미 적용된 patch를 변화 없는 성공으로 처리 | 24/42 |
| patch 전 검색 결과만 뺌 | 15/42 |
| 바뀐 파일의 옛 읽기·검색 결과를 뺌 | 2/42 |
| 같은 결과에 출력 대신 "나중에 바뀜" 표시 | 4/42 |
| 출력은 두고 "나중에 바뀜" 표시만 | 11/42 |

## 결정

planning context를 만들 때, read-only Step의 관찰 결과가 덮는 자원을 그 뒤에 완료된 변경 Step(effect class가
read-only가 아닌 Step)이 바꿨으면 그 `toolOutputs` 항목에 두 필드를 붙인다.

- `supersededBy`: 그 자원을 처음 바꾼 완료 Step의 id
- `supersededMeaning`: 고정 사실 문장 "A later completed Step changed a resource this observation covered, so
  this output may no longer match the current state."

출력은 그대로 둔다.

- 자원은 capability 정의의 resource selector(`argumentPointer`)로 정규화된 인자(ADR-0065의 material 복원)에서 읽는다.
  도메인별 규칙은 없다.
- 덮는다는 판단은 같은 자원이거나 `/`로 나뉜 계층에서 그 아래인 경우다. 계층이 없는 자원은 같을 때만 맞는다.
- 순서는 receipt 완료 순서다. 변경 뒤에 끝난 관찰에는 표시가 붙지 않는다.
- 인자나 자원을 모르는 Step(복원 실패, process 실행 등)은 아무것도 덮지 않는다. `bash -c 'sed -i ...'`처럼 process로
  바꾼 파일은 감지하지 못한다.
- 앞쪽 항목이 바뀌므로 provider prefix cache가 그 지점부터 한 번 다시 계산된다(ADR-0036).

### 출력을 숨기지 않은 이유

처음에는 표를 근거로 출력을 숨겼다(4/42). held-out H7(다른 필드 rename)에서 회귀가 나왔다.

- 이전 빌드는 2/2 모두 남은 사용처 0개로 끝났다.
- 숨김 빌드는 3/3 모두 8~9곳을 남기고 완료를 선언했다.

모델은 처음 한 패키지 검색 결과를 할 일 목록으로 쓰는데, 첫 patch 뒤 그 결과가 숨겨지자 workspace 전체를 다시
검색했다. 그 결과는 `.venv` 매치로 잘려 일부만 보였다. 넓은 관찰은 바뀌지 않은 부분의 유일한 기록이라서 숨기면 안 된다.

## 실행 측정 (deepseek flash, --trust)

| 작업 | 빌드 | 결과 |
| --- | --- | --- |
| H6 (설계 사례) | 이전 | 재적용 0, 28회 / 남은 사용처 10, 14 |
| H6 | 표시 | 재적용 0, 0, 0회 / 3회 모두 같은 읽기 9번 반복(`repeated_finished_action`)으로 멈춤, 남은 사용처 12~20 |
| H7 (held-out) | 이전 | 2/2 완료, 남은 사용처 0 |
| H7 | 표시 | 3/3 완료, 남은 사용처 0, 32~38 call |
| T2 (held-out) | 표시 | 1/2 hidden test 통과, 1회는 `material_rejected`(이전 빌드에도 8번 중 2번) |

재적용 루프는 사라졌다. 하지만 H6 완료율은 아직 오르지 않았다. 남은 원인은 temperature 0에서 같은 읽기를
계속 고르는 것이다. 멈춘 지점 4곳을 재현하면 24번 중 24번이 같은 읽기를 골랐고, 이 변경 전 빌드에도
같은 방식으로 멈춘 실행이 T3·H6에서 8번 있었다. 이건 별도로 다룬다.
