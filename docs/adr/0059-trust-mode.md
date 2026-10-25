# ADR-0059: 승인 없이 시작하는 신뢰 모드

- 상태: Accepted
- 기준일: 2026-10-26
- 변경 분류: 범용 REPL 시작 옵션. 특정 도구·파일·업무 규칙 없음.
- 기존 경계: ADR-0033(승인 종류와 수명), ADR-0058(session 허용 답)
- Core authorization·protocol·SQLite schema·journal 변경: 없음.

## 문제

xgen을 개인 작업용으로 쓸 때, 사용자는 자기 컴퓨터에서 자기 일을 맡기면서도 승인을 계속 답해야 한다.
ADR-0058 뒤에도 session의 첫 요청들에서 종류마다 한 번씩 `a`를 답해야 했다. 2개 저장소 x 요청 3개
session에서 승인은 각각 3회였다. 매번 `/permissions KIND allow`를 네 번 입력하는 방법도 있지만 시작할
때마다 반복해야 한다.

## 결정

`xgen --trust`는 REPL session을 model egress, read, write, execute 모두 `allow`로 시작한다.

- 시작 화면에 `trust mode: model/read/write/execute run without asking (/permissions KIND ask to restore)`를
  쓰고, "boundary마다 승인을 유지한다"는 안내 줄은 쓰지 않는다.
- 상태는 `/permissions KIND allow`를 네 번 입력한 것과 같다. 그래서 `/permissions KIND ask|deny`로
  session 중에 좁힐 수 있다.
- 매번 켜고 싶으면 shell alias를 쓴다. 신뢰 상태를 파일에 저장하지 않는다.

바뀌지 않는 것:

- journal, Receipt 검증, 끝난 뒤 `Changes` 표시(ADR-0057), Ctrl+C, `/resume`.
- 실행 파일 catalog. 이 옵션은 이미 catalog에 있는 것의 승인만 생략한다.
- Core의 action 단위 authorization. 각 effect는 실행 직전에 기존 검증을 거친다.
- 비대화형 `xgen run`/`resume`의 승인 플래그.

## 대안

- **신뢰 workspace 목록을 config에 저장:** 디렉터리마다 다르게 정할 수 있지만 경로 정규화, 하위
  디렉터리 포함, 추가·삭제 명령이 필요하다. 개인 사용에서는 alias로 충분해 보류한다.
- **기본값을 allow로 변경:** 처음 쓰는 사용자가 모르는 채로 모델 전송과 쓰기를 허용하게 된다.

## 검증

- 단위: `--trust` session은 승인 prompt 없이 시작과 재개 모두 grant를 전달한다. `/permissions write ask`
  뒤에는 write만 다시 묻고 `/permissions`가 `write=ask`, 나머지 `allow`를 보인다.
- e2e: 실제 binary를 `--trust`로 실행해 read Step이 있는 goal을 입력만으로 완료한다. `[y/a/N]`이 없고
  Receipt는 1개다.
- 실사용(deepseek): ADR-0058과 같은 2개 저장소 x 요청 3개 session.

  | 저장소 | ADR-0058 이전 (`y`) | `a` | `--trust` | 수정 결과 |
  | --- | --- | --- | --- | --- |
  | graph-tool-call 사본 (설계) | 7회 | 3회 | 0회 | `README.md +2 -0`, 정확 |
  | gwanjong-mcp 사본 (검증용) | 7회 | 3회 | 0회 | `README.md +2 -0`, 정확 |
