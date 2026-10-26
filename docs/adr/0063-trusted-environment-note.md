# ADR-0063: 신뢰 모드 goal에 workspace 경계 안내 붙이기

- 상태: Accepted
- 기준일: 2026-10-26
- 변경 분류: 범용 REPL host 안내. 특정 경로·명령·업무 규칙 없음.
- 기존 경계: ADR-0059(신뢰 모드), ADR-0060(신뢰 모드 shell)
- protocol·SQLite schema·journal·planner prompt 변경: 없음. 안내는 goal 텍스트 뒤에 붙는다.

## 문제

`xgeny.fs/*` capability는 workspace 상대 경로만 받는다. 모델은 workspace가 어디인지, 그 밖은 shell로
가야 한다는 것을 모른다. 그래서 goal이 `~/projects/app`처럼 workspace 밖을 가리키면 그 경로를 workspace
안의 상대 경로로 해석해 `not_found`를 받고 같은 시도를 되풀이했다.

Claude Code 비교 실험의 저장소 상태 조회 작업(`~/projects/app` 아래 저장소 24개)에서:

- 24 turn 예산 2회: shell을 한 번도 쓰지 않았다. `list-directory` 24회 중 17회가 `not_found`였다.
- 96 turn 예산 1회(ADR-0062 뒤): call 96개 중 93개가 같은 `list-directory projects`였다.

## 결정

신뢰 모드 REPL은 shell을 catalog에 넣을 때 goal 뒤에 host 안내 한 단락을 붙인다.

```text
Host environment (trusted local session): the workspace root is "<절대 경로>". The xgeny.fs
capabilities accept only paths relative to it and cannot reach anything outside it; an absolute
path, a ~ path, or a path leaving the workspace fails with not_found there. For locations outside
the workspace, run the `<shell>` executable with arguments ["-c", "<command>"]. The user's home
directory (~) is "<home>".
```

- shell 인자는 bash·zsh·sh면 `-c`, pwsh·powershell이면 `-Command`다.
- 경로는 JSON 문자열로 인용한다. UTF-8이 아니거나 control character가 있으면 안내를 붙이지 않는다.
  home을 모르면 home 문장만 뺀다.
- goal과 안내를 합쳐 `MAX_GOAL_BYTES`를 넘으면 붙이지 않는다.
- 승인 모드에는 붙이지 않는다. shell이 catalog에 없고, workspace 절대 경로를 모델에 보내는 것은 사람이
  단계마다 보는 모드의 기본값으로 두지 않는다.
- 안내는 goal의 일부라 Run manifest·journal에 goal과 함께 남는다. 별도 저장은 없다.

## 대안

- **planner system prompt에 넣기:** 모든 provider·모드·headless Run에 영향을 준다. 이 안내는 신뢰 모드의
  host 사실이라 host가 붙인다.
- **filesystem capability가 workspace 밖을 읽게 하기:** capability 계약과 scope 검사를 바꿔야 한다.
  shell이 이미 있으므로 안내로 충분한지 먼저 본다.
- **`not_found` 결과에 안내 추가:** tool output 계약 변경이다. 실패를 본 뒤에야 알게 된다.

## 검증

- 단위: 안내에 workspace 인용, `not_found`, shell별 인자, home이 들어가고 control character 경로는
  안내를 만들지 않는다.
- 실사용(deepseek flash, `--trust`, 96 turn):

  | 작업 | 안내 없음 | 안내 있음 |
  | --- | --- | --- |
  | 저장소 상태 조회 `~/projects/app` (설계) | shell 사용 0/3, 답 0/3 | shell 사용 2/2(첫 step부터), 답 1/2 |
  | `/etc/os-release` 이름·버전 (검증용) | 1/2 (1회는 절대 경로를 파일 도구로 보내 `invocation_invalid` 3연속) | 2/2 |
  | `~/.config` 항목 수 (검증용) | 2/2 | 2/2 |

  저장소 조회의 답한 1회는 16개를 맞혔고 `.git`이 파일인 worktree 8개를 빠뜨렸다. 답하지 못한 1회는
  첫 명령에서 같은 결과를 받고도 비슷한 명령을 96번 되풀이했다. 안내 전에도 같은 `list-directory`를
  93번 되풀이했으므로 이 반복은 안내와 별개의 문제로 남긴다.
