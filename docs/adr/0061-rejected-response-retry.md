# ADR-0061: 거절된 모델 응답의 제한된 재시도

- 상태: Accepted
- 기준일: 2026-10-26
- 변경 분류: 범용 driver 정책. 특정 모델·작업·업무 규칙 없음.
- 기존 경계: ADR-0038(invocation 진단), ADR-0057, receipt completion check
- protocol·SQLite schema·journal 변경: 없음. 재시도는 기존 model call 예약·settlement를 그대로 쓴다.

## 문제

모델 응답 하나가 거절되면 Run이 바로 멈췄다. 거절은 두 갈래다.

- `model_rejected.planner_invalid_response`: 응답이 형식 계약이나 completion check를 통과하지 못했다.
- `proposal_rejected.*`: Core가 제안을 받아들이지 않았다(scope 밖 인자, 스키마 위반, 없는 capability).

두 경우 모두 거절된 call은 journal에 settle되어 있어서 `/resume`하면 새 call을 예약해 이어 간다.
사용자가 손으로 `/resume`을 입력해야 했을 뿐이다.

Claude Code와 같은 작업 6개를 비교한 실험(devhub-social 사본, deepseek flash, `--trust`)에서 rename
작업(137개 참조, 17개 파일)이 2회 모두 실패했다. 2회 모두 파일 4개를 고친 뒤 응답 하나가
`planner_invalid_response`로 거절되어 멈췄다. 전체 934개 call 가운데 거절된 응답은 3개였고 그중 2개가
이 작업에서 나왔다. 응답 하나가 틀렸다고 이미 진행한 작업을 버리기에는 거절이 드물다.

## 결정

1. `RunDriver`는 거절된 응답을 연속 `MAX_REJECTED_RESPONSE_RETRIES`(2)번까지 버리고 다음 tick을 진행한다.
   대상은 `PlannerUnavailable(InvalidResponse)`와 `ProposalRejected`다.
2. 제안이 받아들여지면(`PlanAccepted`) 연속 횟수를 0으로 되돌린다. 세 번 연속 거절되면 지금처럼 Run을
   멈추고 같은 reason을 보고한다.
3. 재시도마다 새 model call이다. journal에 예약·settle되고 Run의 model call 예산(turn 수의 2배)과
   usage에 그대로 잡힌다. 별도 예산을 두지 않는다.
4. Timeout과 transport 실패는 재시도하지 않는다. 결과가 불확정이라 기존처럼 recovery 경계로 남긴다.
   provider 한도·4xx·rate limit, 그리고 예산 소진 거절(`tool_call_budget_exhausted`,
   `planned_step_budget_exceeded`)도 다시 보내도 결과가 같으므로 재시도하지 않는다.
5. 재시도할 때 거절 이유를 모델 context에 넣지 않는다. 지금 `/resume`과 같은 재표본이다. 이유를 넣으려면
   planning context 계약을 바꿔야 하므로 별도 결정으로 미룬다.

`xgen run`(headless)도 같은 driver를 쓰므로 거절 exit code 20은 세 번 연속 거절된 뒤에 나온다.

## 대안

- **재시도 없이 사용자에게 `/resume` 안내:** 지금 동작이다. 거절이 드문데 사람이 매번 개입해야 한다.
- **무제한 재시도:** 같은 실수를 반복하는 모델이 예산을 다 쓸 때까지 멈추지 않는다.
- **거절 이유를 context에 넣고 재시도:** 수정 가능성은 높지만 context 계약 변경이다. 재표본만으로 충분한지
  먼저 확인한다.

## 검증

- 단위·e2e: 잘못된 응답 하나 뒤의 정상 completion으로 headless Run이 `/resume` 없이 완료된다. 거절·resume
  경로를 검사하던 기존 테스트는 잘못된 응답을 3번 주어 그대로 거절 경로를 검사한다. usage 테스트는
  재시도 call도 집계됨을 확인한다.
- 실사용: 같은 rename 작업을 새 binary로 2회 실행했다. 2회 모두 거절된 응답 2개를 버리고 이어 가서
  정상 제안 24개를 처리했다(call 26개). 더는 거절로 멈추지 않았고, 24 turn 예산을 다 써서 멈췄다
  (참조 137개 중 86개 남음). 예산은 별도 문제다.
