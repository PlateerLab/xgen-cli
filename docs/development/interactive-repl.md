# XGEN 대화형 REPL

## 빠른 시작

프로젝트 root에서 subcommand 없이 실행한다. TTY에서 profile이 없으면 제공자 선택, 숨김 API key
입력, model 선택과 연결 검증을 진행한다. DeepSeek와 OpenAI는 preset으로 URL과 wire 옵션을 채우며,
다른 OpenAI-compatible endpoint는 URL을 직접 입력한다. 저장된 설정은 다음 실행부터 재사용한다.

```bash
cd my-project
xgen
```

API key는 OS credential store에 저장한다. 저장소를 쓸 수 없으면 숨김 입력한 key는 현재 process의
메모리에만 두고 다음 실행 때 다시 묻는다. 평문 key file이나 tool process 환경변수로 옮기지 않는다.
자동화의 `--store-token`은 여전히 저장 실패를 오류로 반환한다.

기본 approval mode는 모두 `ask`다. Model prompt에는 현재 goal, 제한된 이전 요청·답변과 이후 tool
observation이 포함될 수 있다. Read, write와 process execute는 각각 별도로 묻는다.
한 goal을 처리하는 동안 승인한 종류는 이후 continuation에서도 유지하며, 다음 goal이나 명시적인
`/resume`에서는 다시 기본 approval mode를 적용한다. `deny`는 계속 실행을 차단한다.

승인 prompt의 답은 세 가지다([ADR-0058](../adr/0058-session-approval-answer.md)). `y`는 지금처럼 이번
goal에만 허용한다. `a`는 그 종류를 남은 REPL session 동안 `allow`로 바꾸며
`/permissions KIND allow`와 같다. 바뀐 사실과 되돌리는 명령을 한 줄로 표시한다. 그 밖의 입력은 거절이다.
Session 허용은 파일에 저장하지 않고 process가 끝나면 사라진다.

`xgen --trust`는 네 종류를 모두 `allow`로 둔 채 session을 시작해 승인을 묻지 않는다
([ADR-0059](../adr/0059-trust-mode.md)). 시작할 때 한 줄로 알리고, `/permissions KIND ask`로 일부를 다시
묻게 할 수 있다. 기록, `Changes` 표시, Ctrl+C, `/resume`은 같다.
신뢰 모드는 사용자의 shell(`SHELL`이 bash·zsh·sh면 그것, 아니면 PATH의 bash→zsh→sh, Windows는
pwsh→powershell)도 catalog에 넣는다([ADR-0060](../adr/0060-trusted-shell-executable.md)). 모델은 이것으로
파이프와 catalog에 없는 명령을 실행한다.
신뢰 모드의 goal당 model turn 기본값은 24가 아니라 96이다([ADR-0062](../adr/0062-trusted-turn-budget.md)).
`--max-model-turns`를 주면 그 값이 우선한다.
신뢰 모드는 goal 뒤에 workspace 절대 경로, home, workspace 밖은 shell로 가라는 안내 한 단락을 붙인다
([ADR-0063](../adr/0063-trusted-environment-note.md)).

```text
xgen> 프로젝트 구조를 보고 테스트 실패를 수정해줘.
Allow sending the goal, session context, and tool observations to the model? [y/a/N] y
Thinking… · 2s
Allow read for this durable continuation? [y/a/N] a
read allowed for this session (/permissions read ask to undo)
```

줄 끝에 unescaped `\`를 쓰면 다음 줄을 같은 goal로 입력한다.

```text
xgen> src를 탐색하고 \
...> 실패한 테스트만 수정해줘.
```

## 명령

```text
/model [PROFILE]
/status
/permissions
/permissions model|read|write|execute ask|allow|deny
/resume [RUN_ID]
/clear
/exit
```

`allow`는 현재 REPL session에서 사용자가 명시한 선택이다. Process executable은 PATH에서 발견한 고정된
공통 개발 도구 allowlist의 absolute native executable만 catalog한다. `/permissions`는 model에 보이는
logical ID만 출력하며 host path는 출력하지 않는다. Catalog는 실행 권한이 아니고, 실행은 별도 approval과
Core authorization을 거친다. Catalog snapshot은 첫 실제 goal에서 만들어 같은 REPL process의 resume에
재사용하며, 선택된 binary는 매 실행 직전에 다시 검증한다.

## 세션과 재개

한 goal은 하나의 durable Run이다. 일반 질문이나 이전 설명에 대한 후속 질문은 도구 실행 없이 답할 수 있다.
이 경우 `/status`는 `responded`를 표시하고 파일·process 실행 승인을 묻지 않는다. 도구 Step을 계획한 Run은
대화 응답으로 종료할 수 없고, 기존대로 Receipt 검증 뒤 작업 완료 응답을 출력한다.

최근 요청·답변은 최대 8 turn, JSON 기준 12 KiB의 비신뢰 context로 이어진다. 현재 요청은 유지하고 한도에
맞지 않는 오래된 문맥부터 생략한다. 별도 장기 transcript는 저장하지 않지만 다음 Run에 전달한 문맥은
그 Run의 goal에 포함된다. `/clear`는 문맥과 active/last pointer만 제거하며 SQLite Run을 삭제하지 않는다.

같은 세션에서는 `/resume`으로 현재/마지막 Run을 재개한다. 다른 process에서 재개하려면 `/status`의
Run ID를 기록한다. `--debug` 또는 pipe 모드에서는 stderr의 `XGEN_STARTED run_id=...`도 사용할 수 있다.

```text
xgen> /resume run-0123456789abcdef0123456789abcdef
```

작업 완료와 대화 응답을 저장한 Run은 model, workspace 또는 tool effect 없이 summary를 offline replay한다. Approval 대기 Run은
동일한 physical workspace와 자동 catalog snapshot이 필요하다. 도구 binary, PATH 또는 safe environment가
바뀌어 execution profile이 달라지면 configuration mismatch로 fail-closed할 수 있다.

## Progress와 Ctrl+C

일반 TTY 화면은 요청 처리 중 `Thinking…`과 경과 시간을 표시한다. 이 표시는 model 호출뿐 아니라
도구 실행과 결과 검증 시간을 포함하며, model의 내부 reasoning text를 뜻하지 않는다. 승인 입력이나
결과 출력 전에 표시를 지운다. 최종 summary만 durable completion 검증 뒤 control character를 escape해
출력하며, Run ID는 `/status`에서 확인할 수 있다. 이 검증은 Receipt와 결과 저장의 결합·무결성을
확인하며, summary의 모든 자연어 설명이 실제 변경과 일치함을 보장하지 않는다.

```bash
# 기존 durable progress와 Run ID 로그 확인
xgen --debug
```

`--debug`와 pipe 입력에서는 기존 `progress:` event 및 `XGEN_STARTED` 계약을 유지하고 ANSI animation을
출력하지 않는다. Progress는 runtime의 redacted lifecycle event이며 strict JSON proposal은 표시하지 않는다.

`--debug`는 `run`과 `resume`에도 같은 `progress:` event를 같은 formatter로 내보낸다. 전부 stderr의
기존 `XGEN_*` 결과 줄 옆에 쓰므로 stdout은 최종 응답만 담는다. 플래그를 주지 않으면 아무 event도
내보내지 않아서, 결과 줄만 파싱하는 host는 묻지 않은 prefix를 받지 않는다. 진행 표시 쓰기가 실패해도
Run은 계속한다. 진단 한 줄을 잃은 것이 이미 진행 중인 durable 작업을 버릴 이유는 아니고, `run`의
중단은 process-level signal handler가 소유한다.

Ctrl+C는 다음 안전한 durable 경계에서 멈춘다. Model request나 process outcome이 이미 불확정해졌으면
`user_cancelled`보다 `model_call_unknown`/`effect_outcome_unknown`이 우선하고 자동 재실행하지 않는다.
Network/process call이 진행 중이면 configured timeout까지 기다릴 수 있다.

## 긴 결과 보기

일반 TUI는 완료된 답변과 `Changes`를 결과 블록으로 보관한다. 받은 시점의 창 폭으로 12행을 넘으면
앞 4행과 뒤 2행만 표시하고 `Ctrl+O` 안내를 넣는다. 한 줄짜리 긴 문단도 표시 폭을 기준으로 접는다.

- `Ctrl+O`: 최근 결과 전체 보기. 방향키 위·아래, `PgUp`·`PgDn`·Space, Home·End로 이동한다.
- 방향키 좌·우: 보관된 다른 답변·변경 내역으로 이동한다.
- `Esc`, `q`, `Ctrl+O`, `Ctrl+C`, `Ctrl+D`: 전체 보기를 닫고 입력창으로 돌아간다.
  전체 보기에서 `Ctrl+C`는 Run을 취소하지 않는다. 돌아온 뒤 `Ctrl+C`를 누르면 기존 취소 동작을 한다.

전체 보기는 읽기 전용이다. Enter나 붙여넣기로 목표를 보내거나 승인하지 않는다. 열기 전의 입력과 대기열을
유지하고, Run이 계속 진행돼 승인이나 새 결과가 도착하면 아래쪽에 안내한다. `Esc`로 돌아오면 승인 prompt와
새 출력이 나타난다. 전체 보기에만 임시 alternate screen을 사용하고, 닫으면 기존 terminal scrollback으로 돌아온다.

TUI 변경 내역은 파일당 12줄·전체 40줄로 잘린 미리보기 대신 전체 rendered 내용을 보관하므로, 전체 보기에서
그 뒤의 edit·새 파일·교체 파일 내용과 실행 명령 인자도 읽을 수 있다. 원문은 화면 출력용 escape를 거친 내용이다.

캐시는 최근 20개 결과 블록, 원문·행 배열 합계 8 MiB까지다. 오래된 항목은 전체 보기 목록에서 빠지지만
durable Run 기록은 삭제하지 않는다. 한 블록이 한도보다 크면 접지 않고 전체를 출력한다. 전체 보기 중 보류할
새 출력이 8 MiB를 넘으면 전체 보기를 닫고 출력해 내용을 잃지 않는다.

`--debug`, pipe 입력, headless `run`·`resume`은 접지 않는다. 키와 경계는 [ADR-0078](../adr/0078-folded-results.md)을 따른다.

## Headless

Pipe 입력에서는 first-run hidden prompt를 자동으로 열지 않는다. Profile이나 환경변수를 먼저 공급하면
결정적 smoke script를 실행할 수 있다. Secret은 script text나 argv에 넣지 않는다.

```bash
printf '/status\n/exit\n' | xgen
```

실제 goal을 pipe로 실행할 때 원격 HTTPS credential은 `XGEN_OPENAI_API_KEY` 같은 외부 secret injection을
사용한다. 일반 자동화는 exit code와 고정 stderr 계약이 더 단순한 기존 `xgen run/resume`을 권장한다.

대화 응답의 저장·검증 계약은 [ADR-0047](../adr/0047-conversation-response-and-bounded-session-context.md)을 따른다.

## 상태 폴더 권한 오류

Unix에서 `error: state_directory_permissions`가 나오면 함께 표시된 `state_directory`와
`permissions`를 확인한다. 해당 폴더의 group/other 접근을 제거해야 한다.
확인한 경로에 `chmod go-rwx -- PATH`를 직접 실행하면 소유자 권한은 유지된다.
프로그램은 권한을 자동 변경하거나 재귀적으로 고치지 않는다. 경로는 제어 문자를 escape한 인용 표기다.

headless `run`·`resume`·`usage`·`recover`는 같은 code와 진단을 stderr에 내보내고 exit 64로 끝난다.
symlink나 일반 I/O 실패에는 이 복구 안내를 붙이지 않는다. 자세한 범위는
[ADR-0080](../adr/0080-state-directory-permission-diagnostics.md)을 따른다.
