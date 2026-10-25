# ADR-0058: 승인 prompt의 session 허용 답

- 상태: Accepted
- 기준일: 2026-10-26
- 변경 분류: 범용 REPL 표시·입력 계약. 특정 도구·파일·업무 규칙 없음.
- 기존 경계: ADR-0033(REPL 승인 종류와 수명)
- Core authorization·protocol·SQLite schema·journal 변경: 없음.

## 문제

REPL은 요청마다 같은 승인을 처음부터 다시 물었다. ADR-0033의 `ask` 답 `y`는 현재 goal에만 grant를
주고, 다음 goal은 session mode(기본 `ask`)에서 다시 시작한다. 계속 허용하려면 `/permissions KIND allow`를
알아야 하는데, 이 명령은 help에만 있다.

측정(main fcdedb7, deepseek profile):

- 기존 단일 요청 실행 기록 77개: 3회 70개, 2회 6개, 4회 1개. 한 goal 안에서 같은 종류를 두 번 묻는
  경우는 없었다. 3회는 model egress, read, write다.
- 한 session에서 요청 3개(질문 2개, 수정 1개)를 이어서 보냈다. graph-tool-call 사본과 gwanjong-mcp 사본 두
  저장소에서 모두 승인 7회였다. 질문마다 2회, 수정에 3회다.

## 결정

승인 prompt를 `[y/a/N]`으로 바꾼다.

- `y`/`yes`: 기존과 같다. 현재 goal의 continuation에만 grant를 준다.
- `a`/`always`(대소문자 무시): 현재 continuation에 grant를 주고, 그 종류의 session mode를 `allow`로 바꾼다.
  `/permissions KIND allow`와 같은 상태이며 `KIND allowed for this session (/permissions KIND ask to undo)`를
  한 줄 표시한다.
- 그 밖의 입력, 빈 줄, 입력 종료: 거절. 기존과 같다.

바뀌지 않는 것:

- 기본 mode는 모두 `ask`다. session mode는 process 메모리에만 있고 파일에 저장하지 않는다.
- `allow`는 기존과 같이 Core의 action 단위 authorization을 대체하지 않는다. 각 effect는 실행 직전에
  기존 검증을 거친다.
- `xgen run`/`xgen resume`의 비대화형 승인 플래그는 그대로다.

## 대안

- **workspace read 기본 허용:** 승인을 더 줄이지만 보안 기본값을 바꾼다. 별도 결정으로 남긴다.
- **허용을 config 파일에 저장:** 다음 실행까지 이어지지만 저장된 동의의 범위·철회 설계가 필요하다.
- **한 prompt로 model·read·write를 함께 묻기:** 아직 필요하지 않은 종류까지 미리 동의하게 된다.

## 검증

- 단위: `A`·`always`가 session 허용으로 바뀌어 다음 goal에서 해당 prompt가 나오지 않고, `y`로 답한 write는
  다음 goal에서 다시 묻는다. `/permissions`가 `model=allow read=allow write=ask`를 보인다.
- 실사용: 문제 측정과 같은 2개 저장소 x 요청 3개 session에서 처음 나온 prompt마다 `a`로 답했다.

  | 저장소 | 이전 (`y`) | 이후 (`a`) | 수정 결과 |
  | --- | --- | --- | --- |
  | graph-tool-call 사본 (설계) | 7회 (2, 2, 3) | 3회 (2, 0, 1) | `README.md +2 -0`, 정확 |
  | gwanjong-mcp 사본 (검증용) | 7회 (2, 2, 3) | 3회 (2, 0, 1) | `README.md +2 -0`, 정확 |
