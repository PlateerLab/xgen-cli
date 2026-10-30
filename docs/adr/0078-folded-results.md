# ADR-0078: 긴 답변·변경 내역은 접고 읽기 전용 전체 보기를 제공한다

- 상태: Accepted
- 기준일: 2026-10-30
- 변경 분류: 범용 REPL 표시·입력. 모델 입력·journal·protocol·승인·재개 계약 변경 없음.
- 후속 결정: ADR-0074·0077의 result 표시. 일반 TUI는 inline을 유지하고 전체 보기에만 alternate screen을 쓴다.

## 문제

긴 답변은 화면에 모두 흘러가 입력창 위의 맥락을 밀어냈다. 반대로 `Changes`는 파일당 12줄, 전체 40줄로
잘랐고 생략된 내용을 같은 화면에서 펼칠 수 없었다. 단순히 문자열의 `Changes`나 ANSI marker를 찾아 접으면
모델·파일 내용이 표시 제어로 해석될 수 있다.

## 결정

1. REPL writer에 `ReplOutput`의 typed `Answer`·`Changes` section을 둔다. 같은 TUI channel로 전달하므로
   ordinary text·진행·승인과 순서를 유지한다. 모델·파일의 문자열을 보고 section을 추측하지 않는다.
   일반 stdout·test Vec writer는 기존 출력을 유지한다. `--debug`도 section 기능을 끈다.
2. 일반 TUI는 section의 전체 rendered 행을 보관한다. 완료 시 창 폭에서 12행을 넘으면 앞 4행·뒤 2행과
   숨긴 행 수·`Ctrl+O` 안내를 출력한다. 긴 단일 문단과 한글도 표시 폭으로 줄바꿈한다.
   짧은 결과는 기존 styling을 포함한 바이트를 그대로 출력한다.
3. TUI의 `Changes`는 기존 12/40줄 생략 없이, patch edit·생성/교체 파일의 새 내용·실행 인자를 포함해
   section을 만든다. 파이프·debug의 기존 제한 출력은 유지한다. 표시 텍스트는 기존 control escape 경로를 쓴다.
4. `Ctrl+O`는 최근 결과의 읽기 전용 전체 보기를 연다. 위·아래/j/k, PgUp·PgDn/Space, Home·End로 읽고,
   좌·우로 다른 결과를 고른다. Esc/q/Ctrl+O/Ctrl+C/Ctrl+D는 닫기만 한다. Enter·붙여넣기는 승인·goal로 보내지 않는다.
5. 전체 보기만 alternate screen을 사용해 원래 scrollback을 덮지 않는다. 닫을 때 inline pane·입력·queue를
   복원한다. 창 크기 변경은 전체 내용을 다시 줄바꿈하고 선택한 section을 유지한다. 화면이 바뀌지 않으면
   같은 page를 다시 쓰지 않는다. 종료·panic 복구는 alternate screen·cursor·raw mode도 복원한다.
6. Run은 전체 보기를 열어도 계속한다. 승인 대기가 생기면 `Approval pending`, 새 출력이 오면 `New output`을
   알린다. 완료 출력은 보류했다가 닫을 때 순서대로 보인다. 승인하려면 전체 보기를 닫고 기존 prompt에 답한다.
7. 최근 section 20개, 원문·행 배열의 보관 비용 합계 8 MiB를 한도로 오래된 것부터 뺀다. 한 section이
   한도보다 크면 저장하거나 접지 않고 전체 출력한다. 보류 출력이 8 MiB를 넘으면 viewer를 닫고 모두 출력한다.
   이 캐시는 process 안의 표시 캐시이며 durable Run·모델 context·history 파일을 바꾸지 않는다.
   8 MiB는 보관한 rendered 원문과 그 행 배열의 정산 한도다. 현재 viewer의 줄바꿈·page 캐시는 별도 임시
   표시 메모리이므로 process 전체 메모리 상한을 뜻하지 않는다.

## 검증

- 설계 사례는 서로 다른 답변 40행과 생성 파일 변경 60행이다. 두 section 모두 미리보기를 줄이면서
  중간·끝 원문을 전체 보기에서 확인한다. 기존 headless/debug formatter 제한은 별도 assertion으로 유지한다.
- 별도 교체 파일 60행과 한글 단일 긴 문단, 작은 창은 다른 렌더링 사례로 검증한다.
- 전체 보기 frame은 폭 1..80 × 높이 1..24에서 범위 안에 들어가고 선택한 결과를 유지한다.
  cache 수·byte 한도, 초과 시 전체 출력, 짧은 결과 styling, model control character escape를 검사한다.
- `python3 scripts/smoke-inline-tui.py --results`는 실제 Linux/tmux와 loopback provider를 사용한다.
  접힌 답변·변경 내역의 중간/끝, result 이동·창 크기 변경, 원래 scrollback·입력 복원을 확인한다.
  별도 background read 동안 viewer를 열고 승인 대기 Enter 무효, 명시적 승인 뒤 도착한 결과의 보류·복원도
  확인한다. 예상 HTTP model call은 5개이며 viewer 조작으로 재요청하지 않는다. 유료 LLM은 사용하지 않는다.
- 기존 resize/승인 smoke와 새 결과 smoke를 `Fast / Linux` PR CI에서 모두 실행한다.
- 실제 native terminal 관찰은 Linux/tmux 범위이며 다른 OS의 CI build/unit 검사와 구분한다.

## 제한과 후속 과제

빠른 resize 잔상은 이후 [ADR-0079](0079-terminal-screen-size.md)에서 수정했다. 아래는 발견 당시 기록이다.

- 연속 resize stress(`--rapid-resize`)에서는 축소 직후 확대하면 inline pane의 이전 테두리
  한 줄이 남는 경우를 발견했다. viewer가 닫힌 draft·실행 중 queue 두 상태에서 재현했다.
  변경 전 main `ad2a898`을 별도 archive에서 빌드한 바이너리도 같은 40×12 → 100×30 stress에서 실패했다.
  이번 결과 접기와 별도로 기존 cursor 기반 pane 위치 복구를 더 검토해야 한다.
  origin clamp·오른쪽 여백·absolute row 출력 가설은 해결하지 못해 채택하지 않았다.
- PR smoke는 resize 후 완성된 frame·caret이 250ms 유지되는 정상 상호작용을 검사한다.
  rapid stress는 알려진 실패를 재현하는 선택 실행이며 통과를 주장하지 않는다.

- 최종 로컬 회귀는 Rust 773 passed·4 ignored(46 suites), Clippy·fmt 통과다.
- 변경 추적: [PR #135](https://github.com/PlateerLab/xgen-cli/pull/135), 코드 `45c4c0b`,
  [5개 플랫폼을 포함한 전체 CI](https://github.com/PlateerLab/xgen-cli/actions/runs/41055760380).
