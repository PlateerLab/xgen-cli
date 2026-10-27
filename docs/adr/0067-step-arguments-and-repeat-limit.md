# ADR-0067: 인자는 non_idempotent Step에만, 읽기 반복 거절은 따로 8회까지

- 상태: Accepted
- 기준일: 2026-10-27
- 변경 분류: 범용. ADR-0065의 적용 범위와 ADR-0064·0061의 거절 재시도 한도를 고친다. 특정 모델·작업·
  capability 규칙 없음. 다만 한도 8은 아래 Run들에서 측정한 값이다.
- protocol·journal·schema 변경: 없음.

## 문제

ADR-0065 뒤 rename 작업 두 개(T3, 검증용 H6)가 같은 방식으로 멈췄다. 모델이 파일을 읽은 직후 같은 파일을
다시 읽으려 했고, ADR-0064의 읽기 반복 거절이 세 번 이어져 ADR-0061 한도에 걸렸다.

### 어떤 Step의 인자가 다시 읽기를 부르는가

기록된 요청 재생. 마지막 Step이 끝난 읽기인 context 44개(H6·T3 Run 하나씩) x 2회에서 같은 파일을 곧바로
다시 읽은 횟수다(deepseek flash).

| context에 보인 인자 | 같은 파일 다시 읽기 |
| --- | --- |
| 모든 Step (ADR-0065) | 26/88 |
| read_only 제외 | 19/88 |
| 없음 | 7/88 |

읽기 인자와 patch 인자 모두 다시 읽기를 늘렸다. patch 인자가 이미 적용한 patch를 다시 보내는 것을 막는지도
보았다. 실패한 patch 직후 context 20개 x 2회에서 같은 patch를 다시 보낸 횟수는 인자 없음 22·23, 있음 14·23
(Run 두 개)이었다. 실제 Run에서도 patch 인자가 보이는 build에서 같은 patch 53회 재전송이 나왔다. 효과가 없다.
반면 process 실행 인자는 같은 명령 반복을 없앴다(ADR-0065의 T4·H5). terminal 실행도 같은 모양이었다.
인자 지원이 없던 `terminal/start`로 같은 명령을 11번 다시 실행한 Run이 있었다.

### 거절 한도

읽기 반복 거절 뒤 모델이 다른 일로 넘어간 비율은 31회 중 15회였다. 연속 거절 길이를 세면 33번 중 28번이
8회 이내에 끝났다(1회 18, 2회 6, 3회 2, 6·8회 각 1). 나머지(11, 17, 31, 39, 48회)는 사실상 멈춘 상태로
call을 소모했다. 한도가 없으면 T3 한 Run이 model call 192개를 다 썼다.

## 결정

1. runtime은 effect class가 `non_idempotent`인 Step의 인자만 묻는다(process 실행, terminal, web search).
   read_only·idempotent Step과 등록되지 않은 capability에는 인자를 넣지 않는다.
2. CLI는 process·terminal·web search recipe에서 기록된 capability와 같을 때만 인자를 돌려준다.
3. driver는 `repeated_finished_action` 거절을 다른 거절과 따로 센다. 연속 8번(`MAX_REPEATED_ACTION_RETRIES`)까지
   다시 묻고 9번째에 같은 reason으로 멈춘다. 제안이 받아들여지면 두 counter 모두 0이 된다. 다른 거절은 ADR-0061대로
   2번이다.

## 검증

- 단위: terminal recipe의 인자는 기록된 capability로만 다시 읽힌다.
- e2e: 읽기와 git 실행을 한 Run에서 git Step만 `arguments`가 있다. 쓰기 두 개와 실패한 명령 뒤 재시작해도
  명령 인자는 다시 만들어지고 쓰기에는 없다. 읽기 반복 8회 뒤 완료는 exit 0, 9회는 exit 20이다.
- 실사용 (deepseek flash, `--trust`, 이 build):

| 작업 | 결과 |
| --- | --- |
| H5 `package.json` 디렉터리 (검증 사례) | 2 call 완료 2/2 |
| T4 dirty 저장소 | 3 call 완료 |
| T1, T2, T6 | 통과 (T2 숨은 검사 포함) |
| T3 rename (참조 137개) | 0/2. 읽기 반복 9연속으로 멈춤 1, patch 실패 44개로 turn 소진 1 |
| H6 rename (검증 사례) | 0/2. 읽기 반복 9연속으로 멈춤 1, `material_rejected` 1 |

긴 rename 작업은 build마다 결과가 크게 흔들렸다. 같은 두 작업에서 거절 한도가 없던 중간 build는 T3 3/4·H6 1/4를
완료했고 이 build는 0/4다. 표본 2~4개로는 build 차이와 모델 분산을 구분할 수 없어 긴 작업 완료율로 이 결정을
판단하지 않는다. 결정의 근거는 위의 재생 실험(같은 context에서 조건만 바꾼 비교)이다. 긴 작업의 남은 실패는
대부분 patch 작성 실패(`patch_context_ambiguous`·`patch_context_missing`, 한 Run에서 최대 44개)와 모델 JSON 괄호
오류이고, 별도 결정으로 다룬다.
