# ADR-0060: 신뢰 모드의 shell catalog

- 상태: Accepted
- 기준일: 2026-10-26
- 변경 분류: 범용 실행 파일 catalog 정책. 특정 명령·업무 규칙 없음.
- 기존 경계: ADR-0028(shell 없는 process 실행), ADR-0033(개발 도구 allowlist), ADR-0059(신뢰 모드)
- Capability 계약·protocol·SQLite schema·journal 변경: 없음.

## 문제

REPL의 catalog는 PATH에서 찾은 개발 도구 25종 고정 목록이다. 개인 작업(디스크·메모리 확인, 프로세스
조회, HTTP 요청, 파일 집계)에 필요한 `df`, `ps`, `curl`, `find` 같은 명령은 없고 파이프도 쓸 수 없다.
세션 중 `/allow-executable bash=/bin/bash`로 shell을 넣으면 되지만 매번 입력해야 한다.

먼저 이 수동 방식으로 확인했다(main cf60d42, `--trust`, deepseek). 디스크·메모리 요약, 디렉터리 집계,
`curl` 요청 3개 모두 모델이 스스로 `bash -c`로 `df -h`, `find ... | wc -l`, `curl -sS ...`를 실행해
끝냈다. 막힌 것은 shell을 직접 등록해야 한다는 점뿐이었다.

## 결정

`xgen --trust` session은 시작할 때 사용자 shell 하나를 catalog에 더한다.

- 선택: `SHELL`이 절대 경로의 실행 파일이고 이름이 `bash`, `zsh`, `sh`면 그것. 아니면 PATH에서
  `bash`, `zsh`, `sh` 순서로 처음 찾은 것. Windows는 `pwsh`, `powershell`.
- logical id는 파일 이름(`bash` 등)이다. 같은 id가 이미 있으면 더하지 않는다.
- 시작 화면에 catalog(`executables: ... bash`)를 보여 준다.
- 신뢰 모드가 아니면 바뀌지 않는다. 그때는 기존처럼 `/allow-executable`을 쓴다.

바뀌지 않는 것:

- adapter는 ADR-0028대로 shell 없이 argv를 실행한다. shell은 catalog에 있는 실행 파일 하나이고, 모델이
  `bash -c "..."`처럼 인자를 넘긴다.
- 실행 직전 실행 파일 내용 재검증, timeout(최대 10분), 출력 상한(stream마다 32 KiB), no-replay 복구.
- 실행한 명령은 끝난 뒤 `Changes`에 `ran bash -c ... (file changes not tracked)`로 보인다(ADR-0057).

## 대안

- **PATH 전체를 catalog에 넣기:** 수천 개 실행 파일의 내용 digest를 매 session 계산해야 하고 planning
  context도 커진다. shell 하나로 같은 범위를 얻는다.
- **새 `shell/run` capability:** 계약·verifier·fixture를 새로 만들어야 하지만 얻는 것이 기존
  `process/execute` + shell과 같다.

## 결과

- 신뢰 모드의 모델은 사용자 OS 권한으로 무엇이든 실행할 수 있다. 사용자가 `--trust`로 이를 명시한다.
- 명령이 바꾼 파일은 `Changes`에 줄 단위로 나오지 않는다. 실행한 명령만 나온다.

## 검증

- 단위: 알려진 `SHELL`을 우선하고, 모르는 shell(`fish`)이나 상대 경로면 PATH의 `bash`로 돌아가며,
  없으면 넣지 않는다.
- e2e: 실제 binary `--trust` 시작 화면의 catalog에 shell이 있다.
- 실사용(deepseek, `--trust`, `/allow-executable` 없음): 한 session에 4개 요청.

  | 요청 | 결과 |
  | --- | --- |
  | 디스크·메모리 요약 (설계) | `bash -c df -h`, `bash -c free -h`, 정확 |
  | `~/projects/app` 바로 아래 디렉터리 개수 (설계) | `find -maxdepth 1 ...`, 148개로 실제와 같음 |
  | `curl`로 HTTP 요청 (설계) | 1회 `invocation_invalid`로 거절. 새 session 5회 반복은 5/5 성공 |
  | 메모리 상위 프로세스 5개 (검증용) | `ps ... --sort=-rss | head`, 정확 |

  거절 1회는 대화 문맥이 있는 세 번째 요청에서만 나왔고 거절된 제안 내용은 남지 않아 원인을 확인하지
  못했다. 재현되면 planner 안내를 다시 본다.
