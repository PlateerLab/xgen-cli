# ADR-0052: 확정된 read-only Step 실패 뒤 계획 계속

- 상태: Accepted
- 기준일: 2026-10-25
- 변경 분류: 범용 runtime 계약. 특정 도구·파일·업무 규칙 없음.
- 기존 경계: ADR-0015, ADR-0022, ADR-0038
- protocol·journal·SQLite schema 변경: 없음. reducer의 완료 수락 조건만 넓어진다.
- planner prompt 변경: 없음. prompt 바이트는 저장된 request-profile digest에 묶여 있다.

## 문제

Step 하나가 `Failed`가 되면 frontier가 `failed_step_ids`를 보고하고, agent loop는 다음 planning 전에
`FailedSteps`로 멈춘다. public CLI는 이를 `rejected: failed_work`로 끝낸다. 부작용이 없는 읽기
실패도 같은 길을 탄다.

실사용 시나리오에서 75 KB 파일을 `xgeny.fs/read-text`로 읽으려다 64 KiB 상한(ADR-0022)에 걸렸고,
요청 전체가 8초 만에 거절됐다. 없는 경로, UTF-8이 아닌 파일, 일반 파일이 아닌 대상도 같다. 모델이
목록·검색 결과로 다른 경로를 고르거나 실패를 설명할 기회가 없다. 같은 세션에서 테스트 실패(exit 1)는
정상 observation이라 모델이 이어서 고칠 수 있었다. 실패의 종류가 아니라 실패가 어느 계층에서
보고되는지가 결과를 갈랐다.

## 결정

### 1. 확정된 read-only 실패는 settled로 본다

`StepStatus::Failed`이고 durable `EffectIntent.effect_class`가 `read_only`인 Step을
*recoverable read failure*로 분류한다. frontier는 이 Step ID를 `failed_step_ids`에 그대로 남기고,
추가로 `read_only_failed_step_ids`에도 넣는다.

- `Failed`는 adapter의 확정 실패나 검증 실패로만 도달한다. `EffectUnknown`, `Reconciling`,
  `ManualRequired`는 이 분류에 들어가지 않는다. crash 뒤 결과를 모르는 읽기는 기존처럼 manual이다.
- read-only effect는 외부 상태를 바꾸지 않는다. 실패한 읽기를 지나쳐도 되돌릴 부작용이 없다.
- `idempotent`, `non_idempotent`, `compensatable`, `unknown` 실패는 기존처럼 Run을 멈춘다.
  쓰기·실행 실패 뒤 계속하려면 부분 효과의 의미를 따로 정해야 하고, 이 ADR의 범위가 아니다.
  인자가 어긋난 확정 쓰기 실패는 [ADR-0055](0055-recoverable-argument-mismatch-failures.md)가
  settled로 넓힌다. frontier 필드도 그 ADR에서 `settled_failed_step_ids`로 바뀐다.

### 2. agent loop는 read-only 실패만 남았을 때 계속 계획한다

`terminal_quiescence`는 read-only가 아닌 실패가 있을 때만 `FailedSteps`를 반환한다. 실패한 Step은
dependency 관점에서는 계속 `Blocked`다. 따라서 그 Step에 의존하는 기존 Step은 진행되지 않고, 새
proposal이 실패한 Step에 의존하면 기존처럼 `BlockedExistingDependency`로 거절된다.

모델은 planning context의 `steps`에서 그 Step의 `status: failed`, objective, capability를 본다.
실패한 Step에는 toolOutput이 없다. 경로·OS 오류·adapter 내부 분류는 context에 넣지 않는다
(ADR-0022, ADR-0038의 redaction 유지).

### 3. 완료는 모든 Step이 receipt-completed 또는 recoverable read failure일 때 수락한다

`WorkFrontier::all_steps_settled()`는 Step이 하나 이상 있고, 모든 Step이 receipt-completed이거나
recoverable read failure일 때 참이다. `TaskCompletion` 수락(reducer와 agent loop 양쪽)과 planned-step
budget 판정은 이 조건을 쓴다. `Conversation` 응답은 기존처럼 Step이 없을 때만 허용한다.

reducer 조건이 넓어지기만 하므로 기존 journal은 새 binary에서 그대로 replay된다. 새 조건으로 수락한
완료가 있는 journal은 이 변경 이전 binary에서 replay되지 않는다.

## 결과

- 읽기 실패 하나로 요청 전체가 끝나지 않는다. 모델은 다른 읽기를 계획하거나 실패를 완료 요약에서
  설명할 수 있다.
- 실패한 읽기도 tool-call과 planned-step budget을 소비한다. 반복 실패는 예산 소진으로 끝난다
  (ADR-0015).
- 실패 원인 분류는 이 ADR에서 모델에 전달하지 않는다. 값 없는 분류 code 전달은
  [ADR-0054](0054-value-free-step-failure-codes.md)가 추가한다.
- 64 KiB를 넘는 파일을 읽는 방법은 이 ADR에 없다. 줄 범위 읽기는
  [ADR-0053](0053-bounded-line-range-text-read.md)이 추가한다.

## 검증

- frontier 분류: read-only 실패는 `read_only_failed_step_ids`에 들어가고 쓰기 실패는 들어가지 않는다.
- reducer: read-only 실패와 receipt-completed Step만 있으면 `TaskCompletion`을 수락하고, 쓰기 실패가
  있으면 거절한다.
- agent loop: read-only 실패 뒤 `FailedSteps` 대신 다음 planning으로 진행하고, 쓰기 실패 뒤에는
  기존처럼 `FailedSteps`로 멈춘다.
- 실사용: 64 KiB 초과 파일이 있는 저장소에서 같은 요청이 `failed_work` 없이 최종 응답까지 간다.
