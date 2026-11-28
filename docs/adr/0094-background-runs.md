# ADR-0094: 대화형 세션의 백그라운드 Run

- 날짜: 2026-11-28
- 상태: 채택, 구현·로컬 검증 완료
- 범위: 대화형 세션(REPL/TUI)의 `/bg`, `/runs`, `/show`, `/cancel` 명령과 agy 취소 범위.
  journal, Run 상태, planning context, 모델 요청, headless `xgen run`/`resume`은 바꾸지 않는다.

## 근거

대화형 세션은 Run 하나를 REPL 스레드에서 끝까지 동기로 실행했다. 하나가 도는 동안 다른 독립 작업은
Run이 끝나기를 기다리거나, 터미널을 하나 더 열어야 했다. Run 중 입력은 [ADR-0092](0092-follow-up-instructions-during-a-run.md)
이후 그 Run의 지시가 된다.

Runtime은 이미 병렬을 막지 않는다.
- Lease는 workspace가 아니라 Run마다 잡힌다(`runs/<id>/run.lock`).
- 파일 쓰기는 `expectedDigest`로 미리 확인한다. 그래서 두 Run이 같은 파일을 고쳐도 내용이 섞이지 않고 뒤에 쓰는 쪽이 실패한다.

## 결정

1. `/bg GOAL`은 독립 Run을 별도 스레드에서 시작한다.
   - 지침 파일과 환경 안내는 일반 goal과 같이 붙인다. 이전 대화 맥락은 넘기지 않는다.
   - Run 중 입력창의 후속 지시(ADR-0092)는 앞 Run에만 간다.
2. 백그라운드 Run은 묻지 않는다.
   - 세션이 이미 허용한 권한만 쓴다. 새 승인이 필요하면 `Paused`로 멈추고 알림으로 `/resume RUN_ID`를 안내한다.
   - 이어 가는 일은 앞(foreground)에서 기존 승인 흐름으로 한다.
   - 입력창이 비어 있을 때의 `/bg`만 모델 전송 동의를 그 자리에서 묻는다.
   - Run 중 입력한 `/bg`는 대기열에 들어가지 않고 바로 실행된다. 이때 질문할 수 없으므로 모델 권한이 `allow`여야 한다.
3. 끝나면 결과 종류와 Run ID, 답 첫 줄을 한 줄로 알린다.
   - TUI는 매 tick 알림을 입력창 위에 찍는다. TUI가 없는 REPL은 다음 입력 전에 찍는다.
   - `/show ID`는 세션 메모리에 저장한 답(최대 64 KiB)을 보여 준다. `/runs`는 목록을 보여 준다.
   - 목록은 32개까지 두고, 끝난 것 중 가장 오래된 것부터 버린다.
4. 동시에 3개까지 돈다.
   - `/cancel ID`는 그 Run의 driver 경계에서 멈춘다.
   - Ctrl+C는 앞 Run만 취소한다. agy의 process 전역 취소 플래그는 백그라운드 스레드에 적용하지 않는다(thread-local 분리).
5. 세션을 끝내면 도는 백그라운드 Run을 모두 취소하고 최대 10초 기다린 뒤 나간다. 각 Run의 durable 상태는 남는다.
6. 입력마다 모델이 "후속 지시인지 새 Run인지"를 고르는 상위 Router는 만들지 않는다.
   - 매 입력에 모델 호출이 하나 늘고, 잘못 분류하면 되돌리기 어렵다.
   - 사용자가 `/bg`로 명시한다. 실제 사용에서 잘못 넣는 경우가 반복되면 다시 검토한다.

## 검증

- 단위 테스트
  - 두 Run이 서로 독립으로 끝나는 순서와 알림, `/show`·`/runs`, `/cancel` 대상 한정, 상한 3, 거절 메시지,
    터미널 제어 문자 제거.
  - REPL: `/bg`, `/runs`, 앞 goal, `/status`, `/exit` 시 취소와 알림.
  - agy 취소의 스레드 분리.
- tmux 대화형, agy `gemini-3.8-flash-low`
  - trust: 앞 Run(파일 2개 설명)이 도는 중에 `/bg`(CSV 행 수)를 입력했다. 백그라운드가 먼저 끝나 `2`를 답했고, 앞 Run도 끝났다.
  - 기본 모드: `/bg` 동의 뒤 읽기 승인 단계에서 `paused (read_approval_required)` 알림이 떴다. `/resume`으로 앞에서 승인하자 답이 나왔다.
  - 같은 파일 충돌: 앞 Run과 백그라운드 Run이 `notes.md`에 서로 다른 줄을 덧붙였다.
    - 백그라운드의 첫 쓰기가 journal에 `effect_failed{precondition_conflict}`로 남았다.
    - 이후 다시 읽어 두 줄이 모두 남았다.
  - Ctrl+C 분리: 앞 Run을 4초 뒤 취소했다.
    - 수정본에서는 백그라운드 Run이 2/2 완료했다.
    - 분리 줄만 뺀 대조 빌드에서는 0/2였다. 두 번 모두 `model_call_unknown.transport_unavailable`로 함께 끊겼다.
- 모델 요청 비교: rc.9 binary와 8개 사례 모두 차이 0.

## 한계

- 백그라운드 Run의 in-flight agy 호출은 `/cancel`로 바로 끊지 못하고, 호출이 끝난 뒤 경계에서 멈춘다.
- 백그라운드 Run은 Step 단위 진행(`read ...`)을 보여 주지 않고 끝날 때만 알린다.
- `/show`는 세션 메모리에 있는 답만 보여 준다. 세션이 끝나면 Run ID로 `/resume`해야 다시 볼 수 있다.
