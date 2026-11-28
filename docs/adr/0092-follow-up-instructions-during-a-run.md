# ADR-0092: Run 중 후속 지시

- 날짜: 2026-11-28
- 상태: 채택, 구현·로컬 검증 완료
- 범위: 대화형 터미널 세션. journal 이벤트 1종(`user_instruction_added`), agent loop 상태 필드 1개,
  planning context 필드 1개(`userInstructions`), 입력창의 Enter 동작. headless `xgen run`·`resume`은 바꾸지 않는다.

## 근거

ADR-0074의 입력창은 Run 중 입력을 대기열에 넣고 Run이 끝난 뒤 다음 목표로 보냈다. 그래서 Run 방향을 바꾸려면
끝나기를 기다리거나 Ctrl+C로 취소하고 다시 시작해야 했다. Claude Code·Codex CLI처럼 실행 중인 작업에 지시를
더할 수 있어야 한다(2026-11-19 사용자 승인 작업 순서).

## 결정

1. Run 중 Enter로 보낸 줄은 그 Run의 후속 지시가 된다. `/`로 시작하는 줄은 명령이라 전과 같이 다음 prompt
   대기열로 간다. 입력창에는 `next step: ...`(아직 전달 전)과 `queued: ...`(다음 목표)로 나눠 보인다.
2. 지시는 공유 inbox(`FollowUpInbox`)에 쌓이고, driver가 다음 모델 호출 직전(`ModelCallStarting` 경계, 호출
   예약 전)에 `UserInstructionAdded { text }`로 journal에 남긴다. 진행 중인 모델 호출의 context는 바뀌지 않으며,
   이미 예약된 호출이 낡은 head로 정산되는(StaleHead) 일이 없다. 도구 실행 중에 보낸 지시는 다음 모델 호출까지
   기다린다. 전달되면 화면에 `↳ added to the run: ...`이 남는다.
3. workgraph는 agent loop가 구성되고, 진행 중 호출이 없고, 완료가 기록되지 않은 Run에서만 받는다. 한 줄
   8 KiB, Run당 16개까지다. 빈 줄·초과는 `InvalidUserInstruction`으로 거절한다. driver는 거절될 지시를 보내지
   않고 inbox에 남긴다.
4. 상태는 `AgentLoopState.user_instructions`(비어 있으면 직렬화하지 않음)이고, planning context는
   `verifiedCompletedSteps` 뒤, `previousResponseRejection` 앞에 `userInstructions { meaning, items }`를 넣는다.
   비어 있으면 필드가 없어 기존 Run의 요청·context digest·projection 바이트가 같다. 고정 문장 `meaning`이
   해석을 정한다: goal을 쓴 사용자가 실행 중에 더한 지시이고 goal의 일부이며, 충돌하면 나중 것이 이긴다.
   system prompt와 request profile은 바꾸지 않아 진행 중인 Run의 재개가 깨지지 않는다.
5. Run이 지시를 받지 못하고 끝나면(완료·승인 대기 뒤 거절·한도) 남은 지시는 다음 `xgen> ` prompt에서 입력
   순서대로 목표가 된다. 승인 질문의 답으로는 절대 쓰지 않는다. Ctrl+C와 Esc는 대기열과 함께 비운다.
   후속 지시도 입력 기록에 남는다.
6. 지시는 journal에 있으므로 재개·오프라인 재생이 같은 context를 다시 만든다. 이 이벤트를 쓴 Run은 이전 바이너리로
   열 수 없다(알 수 없는 이벤트로 무결성 오류).

## 검증

- 단위: workgraph 전이(구성 전·호출 중·완료 뒤·빈 줄·8 KiB 초과·17번째 거절), driver(첫 호출 중 넣은 지시가 둘째
  호출 context에만 보이고, journal 1건, SQLite 재개 상태 동일, 완료 뒤 지시는 inbox에 남음), 입력창(Run 중 입력은
  inbox, `/` 명령은 대기열, 남은 지시가 다음 목표로 먼저 실행, Esc 비우기, 승인 답에 안 쓰임).
- TUI smoke(Fast/Linux CI): Run 중 입력이 `next step:`으로 보였다가 둘째 요청의 `userInstructions`로만 가고,
  모델 요청 3회에서 2회로 줄었다.
- 모델 요청 비교(`scripts/diff-model-requests.py`, main 대비): 8개 사례 모두 차이 0.
- 실제 모델(agy Flash Low, tmux 입력창, `--trust`), 같은 입력을 main과 비교:

| 사례 | 목표 / 후속 지시(목표 3~12초 뒤) | 이 빌드 | main |
|---|---|---|---|
| A 설계 | 모든 파일 한 줄 설명 / Python 파일만 | 목록 뒤 지시 반영, `.py` 2개만 읽고 설명(Run 1개) | 4개 모두 설명 후 지시를 별도 Run으로 실행(1회는 agy 호출 실패) |
| B 설계 | hello.py 생성 / 이름을 greet.py로, 문구 변경 | 같은 Run에서 greet.py 생성(20초) | Run 2개(16초+13초) |
| C 보류 | 파일별 줄 수와 합계 / 답을 한국어로 | 한국어 답 | - |
| D 보류 | README·calc.py 요약 / calc.py에 맞는 함수 하나 제안(12초 뒤, 도구 실행 중) | 두 파일 읽은 뒤 전달, 요약과 제안 함께 | - |

  네 Run 모두 journal에 `user_instruction_added`가 1건이고, C를 `xgen resume`으로 오프라인 재생하면 같은 답이 나왔다.
  B에서 지시 전에 이미 쓴 hello.py는 남았다. 지난 단계는 되돌리지 않는 것이 이 설계다.

## 한계

- 지시는 모델 호출 경계에서만 들어간다. 오래 걸리는 도구 실행이나 승인 대기 중에는 바로 반영되지 않는다.
- 사례마다 1회 실행이라 품질 비율이 아니다. 실제 모델은 trust 모드로만 봤고, 승인 대기를 거친 뒤의 전달은 가짜 모델 TUI smoke(비trust, 모델·읽기 승인)로 확인했다.
- headless에는 입력 경로가 없다.
