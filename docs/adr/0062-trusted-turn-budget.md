# ADR-0062: 신뢰 모드의 기본 turn 예산

- 상태: Accepted
- 기준일: 2026-10-26
- 변경 분류: 범용 REPL 기본값. 특정 작업·도구·업무 규칙 없음.
- 기존 경계: ADR-0059(신뢰 모드), ADR-0061(거절된 응답 재시도)
- protocol·SQLite schema·journal 변경: 없음. 예산은 지금처럼 Run manifest에 기록된다.

## 문제

REPL goal 하나는 기본 24 model turn을 쓴다. 승인을 묻는 기본 모드에서는 사람이 단계마다 보고 있으므로
짧은 예산이 비용과 폭주를 막는다. 신뢰 모드(ADR-0059)는 사람이 단계마다 보지 않는 개인용 모드라 같은
예산이 일을 끝내기 전에 멈추는 원인이 됐다.

Claude Code 비교 실험(devhub-social 사본, deepseek flash, `--trust`)에서:

- rename 작업(참조 137개, 17개 파일): 거절 재시도(ADR-0061)를 넣은 뒤 2회 모두 정상 제안 24개를
  처리하고 `model_turn_budget_exhausted`로 멈췄다. 참조 86개가 남았다.
- 저장소 상태 조회 작업(`~/projects/app` 아래 저장소 24개): 2회 모두 예산을 다 쓰고 답하지 못했다.

두 작업은 서로 다른 종류(편집, 조회)인데 모두 24 turn 안에 끝나지 않았다. 반면 버그 수정, 기능 추가,
후속 대화, 새 스크립트 작업은 24 turn 안에 끝났다.

## 결정

1. `xgen --trust`의 goal당 기본 model turn을 96으로 한다. 기본 모드는 24 그대로다.
2. `--max-model-turns N`을 주면 모드와 관계없이 그 값이 우선한다.
3. 96은 foreground 호출의 tick 상한(`MAX_TICKS` 1,024, turn당 10 tick) 안에 들어가는 값이다. 이 관계는
   컴파일 시점에 검사한다.
4. 나머지 예산(model call, planned step, tool call)은 지금처럼 turn 수에서 2:1:1로 정해진다. context
   상한(512 KiB)은 바꾸지 않는다.

## 대안

- **기본 예산을 모든 모드에서 올리기:** 승인 모드의 비용 상한까지 바뀐다. 필요성을 보인 것은 신뢰 모드뿐이다.
- **예산이 끝난 Run을 `/resume`으로 연장:** manifest의 예산을 바꾸는 계약 변경이라 이번 범위에 넣지 않았다.
- **예산 제한 없음:** 같은 실수를 반복하는 모델이 끝없이 call을 쓴다. 상한은 남긴다.

## 검증

- `xgen --help`가 `[default: 24, or 96 with --trust]`를 보인다.
- rename 작업을 새 binary로 2회 실행:
  - 1회는 164초에 완료했다. 테스트 47개가 통과했고 참조 137개 중 3개가 남았다(call 61개, 거절 7개
    재시도 후 진행).
  - 1회는 215초 동안 정상 제안 70개를 처리한 뒤 존재하지 않는 capability를 3번 연속 요청해
    `proposal_rejected.capability_unavailable`로 멈췄다. 예산이 아닌 다른 문제다. 거절 이유를 다음
    context에 넣지 않아 같은 요청이 반복된 것으로 보이며 후속 과제로 남긴다.
- 저장소 상태 조회 작업은 workspace 밖 경로 안내 변경과 함께 다시 잰다.
