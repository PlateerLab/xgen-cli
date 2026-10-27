# ADR-0064: 거절 이유와 반복 사실을 planning context에 넣기

- 상태: Accepted
- 기준일: 2026-10-26
- 변경 분류: 범용 planning context·Core 규칙. 특정 모델·작업·업무 규칙 없음.
- 기존 경계: ADR-0036(prefix 순서), ADR-0055(`failureMeaning`), ADR-0061(거절 응답 재시도)
- protocol 변경: planning context에 선택 필드 3개(`previousResponseRejection`, Step의 `repeatsStep`·`repeatMeaning`)가
  붙는다. 값이 없으면 직렬화하지 않으므로 기존 context와 바이트가 같다. profile version은 v3 그대로다.
- journal 변경: `ModelCallSettled`의 `Rejected` settlement에 선택 필드 `proposal`(Core 거절 code)이 붙는다.
  없으면 직렬화하지 않으므로 기존 journal을 그대로 읽는다. model call 상태에 `last_rejection`이 생긴다.

## 문제

ADR-0061은 거절된 응답을 2번까지 다시 묻지만 이유는 알려 주지 않았다. 모델은 같은 실수를 되풀이할 수 있다.

더 큰 문제는 반복이다. `~/projects/app` 아래 dirty 저장소 찾기(정답 24개, `--trust`, deepseek flash)에서 모델이
같은 `bash` 명령을 54~72번 다시 실행하며 96 turn을 다 쓰는 경우가 나왔다. rename 작업에서도 이미 읽은 파일을
같은 인자로 세 번 다시 읽었다. context에는 같은 결과가 이미 있었지만 모델이 그것이 반복이라는 사실을 직접
비교해야 했다.

## 결정

1. **거절 이유 전달.** 바로 앞 model call이 거절되었으면 다음 context에 `previousResponseRejection`
   (`reason`, Core 거절이면 `proposalRejection` code, 고정 영어 문장 `meaning`)을 넣는다. 새 call을 예약하면
   지우고, 받아들여지면 비운다. stale head 거절은 모델 잘못이 아니므로 넣지 않는다. 문장은 code마다 정해져
   있고 경로·인자 같은 값을 담지 않는다.
2. **반복 표시.** 끝난(Completed·Failed) Step이 앞서 끝난 Step과 capability·contract·`planInputDigest`가 같으면
   `repeatsStep`에 앞 Step ID를, `repeatMeaning`에 고정 문장을 붙인다. effect class와 상관없이 사실로 표시한다.
3. **읽기 반복 거절.** read-only capability를 마지막으로 끝난 Step과 같은 인자로 바로 다시 제안하면
   `proposal_rejected.repeated_finished_action`으로 거절한다. 사이에 다른 Step이 끝났으면 거절하지 않는다
   (대상이 바뀌었을 수 있다). 쓰기·프로세스 실행은 같은 입력이라도 결과가 달라질 수 있으므로 거절하지 않는다.
   이 거절은 ADR-0061 재시도 대상이며 다음 call은 1번의 이유를 받는다.

## 대안

- **반복 표시만 하고 거절하지 않음:** 측정에서 모델이 `repeatsStep`을 자주 무시했다(같은 명령 54~72회).
- **모든 effect class에 거절 적용:** 반복 루프는 29~42초 만에 끊겼지만(이전 230초), 같은 patch를 다시 적용해
  실패를 확인하던 작업이 회귀했다. 실패를 보고 다시 시도하는 쓰기 반복은 정상 경로일 수 있다.
- **이유를 넣지 않는 재표본(ADR-0061 그대로):** 같은 거절이 이어져 세 번째에 멈춘다.

## 검증

- 단위: 반복 Step 표시(같은 digest는 처음 끝난 Step을 가리킨다), read-only 즉시 반복 거절과 거절 code 기록.
- e2e: 잘못된 응답 뒤 재시도 요청에 `previousResponseRejection.reason`이 `planner_invalid_response`로 들어간다.
  capability 거절은 `capability_unavailable`, 스키마 거절은 `invocation_invalid`가 들어간다.
- 실사용: 아래 측정.

## 측정 (2026-10-26, deepseek flash, `--trust`, 작업당 2회)

| 작업 | 1회 | 2회 | 비고 |
| --- | --- | --- | --- |
| T2 dedupe | 통과 (28초) | 실패: `materialization_failed` | 모든 class 거절 때의 회귀(같은 patch 재적용 거절)는 사라졌다. 실패는 이 결정과 무관한 material 오류다. |
| T3 rename | 참조 137개 중 2개 남음 (140초) | `planner_invalid_response` 3연속으로 멈춤 (60개 남음) | 1회에서 읽기 반복 거절 1번 뒤 이어서 진행했다. invalid response는 변경 전 build에서도 같은 작업에서 run당 2~7개 나왔다. |
| T4 dirty 저장소 | 16/24, 8초 | 같은 `bash` 명령 반복으로 96 turn 소진 (217초) | `bash`는 non_idempotent라 거절 대상이 아니다. 1회는 worktree(`.git`이 파일)를 놓쳤다. |
| T1 | 통과 (15초) | | |
| T6 | 스크립트 생성, `PYTHONPATH` 없이 실행하면 import 실패 | | 모델 품질 문제, 반복·거절 없음 |

해석: 이 결정으로 생긴 회귀는 보이지 않는다. 읽기 반복 거절은 짧은 루프를 끊지만, 측정에서 가장 긴 루프는
프로세스 실행 반복이었고 이 결정은 그것을 막지 않는다. 같은 명령 반복은 결과를 비교해야 판단할 수 있어
다음 결정으로 남긴다. 원인은 끝난 Step의 인자가 context에 없던 것이었다([ADR-0065](0065-finished-step-arguments.md)). 거절 이유가 다음 응답을 고치는지는 표본이 작아 판단하지 않는다.
