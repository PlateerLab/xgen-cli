# ADR-0049: command-scoped PTY terminal session

- 상태: 채택
- 날짜: 2026-10-05
- 범위: 일반 기능. 특정 프로젝트·명령·출력 문구에 대한 engine 분기는 없다.

## 문제와 결정

동기 process 도구는 완료까지 기다린 뒤 stdout/stderr를 반환한다. 장시간 실행과 대화형 프로그램에는 시작·조회·입력·종료를 분리한 PTY session이 필요하다. 별도 agent loop나 approval runtime 없이 기존 catalog·승인·material recipe·durable tool output·Receipt 경로 위에 native capability 4개를 추가한다.

| Capability | 입력 | 효과 |
| --- | --- | --- |
| `xgen.terminal/start@1.0.0` | 기존 process의 executable, args, cwd, env, timeoutMs, maxOutputBytes | catalogued executable을 shell 없이 PTY에서 시작하고 opaque handle 반환 |
| `xgen.terminal/read@1.0.0` | sessionId, offset, maxBytes, waitMs | stdout/stderr가 합쳐진 PTY ring의 비소비 snapshot |
| `xgen.terminal/write@1.0.0` | sessionId, input | literal 입력 전송, 전송한 prefix의 acceptedBytes 반환 |
| `xgen.terminal/terminate@1.0.0` | sessionId | 소유한 process group에 SIGKILL을 보내고 leader 회수 |

Linux의 `portable-pty 0.9.0`과 `nix 0.28`을 사용한다. 이전 probe에서 library의 `Child::kill()`만으로는 서로 다른 두 process tree의 descendant가 남았다. production은 unreaped leader를 `waitid/WNOWAIT`로 확인한 다음 owned group에만 신호를 보낸다. group 소유권을 해제한 뒤에는 PID를 다시 신호 대상으로 쓰지 않는다. macOS/Windows에서는 이 session capability를 제공하지 않는다. 일반 process 도구는 계속 제공한다.

## Lifetime와 복구

Registry는 한 `run`/`resume` 호출의 실행 구간에 묶인다. 대화형 host가 executable/environment snapshot을 재사용하더라도 호출마다 live registry를 새로 만든다. 작업 완료, pause, interrupt, 오류 또는 host 종료 시 소유 group을 정리한다. PTY를 계속 실행하는 daemon이나 OS 재시작 후 재접속 기능은 없다. 이 도구로 시작한 서비스가 최종 응답 후에도 실행된다고 주장하면 안 된다.

한 registry에 생성 가능한 session은 최대 8개다. session lifetime은 100–600000ms, terminal 크기는 80×24다. 시간 제한은 별도 watchdog이 감시하므로 모델 호출이 대기 중이어도 적용한다. 종료된 session의 관측 자료는 registry lifetime 동안 유지하며 동일 start handle은 다시 spawn하지 않는다.

Cold resume에서 과거 handle의 실제 session이 없으면 `session_lost`를 관측 결과로 반환한다. start나 write의 실행 여부가 불확실한 intent는 기존 Unknown/manual-intervention 경로를 따른다. stable-key reconciliation을 지원한다고 주장하지 않으며 자동 재실행하지 않는다. 완료된 run은 저장된 최종 응답만 replay한다.

## 승인·출력 계약

네 capability는 보수적으로 `NonIdempotent`·`Once`·Local boundary를 사용하고 기존 execute 승인으로 제한한다. start의 resource는 catalog-bound `process.execute` executable이다. 다른 operation은 `terminal.session`의 정확한 opaque handle을 사용한다. 모델이 제공한 PID나 host path로 session을 선택하지 않는다. Registry에 없는 handle은 기존 process에 접근하지 않는다.

recipe는 `xgen.cli.terminal-recipe/v1` domain으로 process/search와 분리한다. definition·instance·binding·host policy·recipe version을 execution profile에 포함한다. Linux에서 executable catalog를 사용하는 이전 incomplete run과는 profile이 달라져 호환성을 확인하지 않은 자동 resume를 거절한다. 완료된 응답의 offline replay는 유지한다.

출력은 sessionId, state, exitCode, output, baseOffset, nextOffset, truncated, acceptedBytes다. byte offset은 UTF-8 문자열 길이와 다르다. decoding은 replacement 방식이며 chunk 경계에서 잘린 code point가 replacement 문자로 표시될 수 있다. PTY 원문과 ANSI/control bytes는 기존 untrusted tool output 경로에만 들어간다. 사용자 terminal에 raw output을 직접 render하지 않는다.

출력 ring은 요청한 1024–32768 bytes로 제한한다. overflow 후에도 reader가 계속 drain하며 read는 잃은 bytes를 truncated로 표시한다. read는 최대 4096 bytes, wait는 최대 1000ms다. nextOffset은 반환된 bytes 바로 다음 위치다. output 없는 start/write/terminate snapshot은 baseOffset을 사용하므로 마지막 read의 cursor를 별도로 유지한다.

write는 최대 1024 UTF-8 bytes이고 nonblocking 100ms deadline을 사용한다. acceptedBytes는 kernel PTY에 제출한 prefix이며 프로그램이 소비했다는 증거는 아니다. partial 입력 전체를 blind retry하지 않는다. `stopping`은 leader 회수 대기, `draining`은 마지막 출력 수집 대기다. settled state는 capture EOF와 leader 회수 후에만 표시한다. 소유권이나 cleanup을 확인할 수 없으면 Unknown으로 전달한다.

Process group은 sandbox가 아니다. 새 session/group으로 탈출한 process를 추적하는 기능, resize, persistent daemon, macOS process ownership 및 Windows Job Object/ConPTY 지원은 별도 후속 범위다.

## 검증

영어·한국어 input/output, literal argv, byte cursor, ring overflow, control bytes, write deadline, nonzero exit, timeout, session loss, 반복 start, session 한도를 offline 검증한다. 명시적 종료·leader 정상 종료·host drop에서 descendant 정리를 각각 확인한다. 공개 CLI fixture는 execute 승인 전 시작 0회, 승인 후 Receipt, 정확한 최종 응답과 offline replay 불변성을 검사한다.

실제 모델 평가와 한계는 [개발 기록](../development/pty-terminal-sessions-2026-10-05.md)에 남긴다.
