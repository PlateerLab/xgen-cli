# ADR-0074: 대화형 REPL에 하단 고정 입력창을 둔다

- 상태: Accepted
- 기준일: 2026-10-28
- 변경 분류: 범용 REPL 표시·입력. protocol·journal·planning context·REPL 명령 변경 없음.
- 대체: ADR-0071의 rustyline editor

## 문제

ADR-0071의 줄 편집은 REPL이 줄을 기다릴 때만 동작했다. Run이 도는 동안에는 입력할 곳이 없고, 진행 줄과 결과가
prompt와 같은 흐름에 섞였다. Claude Code·Codex처럼 입력창은 화면 아래에 고정되고 진행과 결과는 그 위로
흘러가야, 긴 Run을 보면서 다음 목표를 미리 쓸 수 있다.

## 결정

1. stdin·stdout·stderr가 모두 터미널이면 `tui` frontend thread가 터미널을 혼자 소유한다(raw mode, bracketed
   paste). alternate screen은 쓰지 않으므로 지난 출력은 터미널 scrollback에 그대로 남는다.
2. REPL 코드는 그대로 둔다. REPL이 쓰는 바이트는 `TuiOutput`을 거쳐 같은 순서로 frontend에 간다. frontend는
   터미널처럼 해석한다: 끝난 줄(`\n`)은 마지막 `\r` 뒤 내용만 scrollback으로 올리고, 아직 끝나지 않은 줄(prompt,
   `\r`로 덮어쓰는 진행 줄)은 하단 pane에 보인다.
3. 줄 요청은 `InterruptibleInput::from_frontend`의 request closure가 출력과 같은 channel로 보낸다. 그래서 frontend가
   요청을 받을 때는 그 앞의 prompt가 이미 도착해 있다. 응답은 기존 `InputMessage`(`Line`, `Interrupted`,
   `TooLarge`, `EndOfInput`, `Unavailable`)를 그대로 쓴다.
4. pane 구성: 진행 줄(spinner `|/-\`) 또는 승인 질문, 대기 중인 목표, 입력 상자, 작업 디렉토리와 키 안내.
5. 키
   - Enter: 보내기. Run 중이면 대기열에 넣고, 다음 `xgen> ` prompt에서 보낸다. 승인 질문에는 대기열을 쓰지 않는다.
   - Alt+Enter, Shift+Enter, Ctrl+J: 줄바꿈. 여러 줄 목표는 기존 `\` 이어 쓰기로 바꿔 보낸다.
   - Ctrl+C: Run 중이면 `Cancellation::request()`(기존 SIGINT 경로와 같음)와 대기열 비우기, 입력 중이면 입력 지우기,
     빈 입력이면 `Interrupted`. raw mode라 SIGINT는 오지 않는다.
   - Ctrl+D(빈 입력): 끝. Esc(Run 중): 대기열 비우기.
   - 편집: 좌우, Home/End, Ctrl+A/E/U/K/W, 위아래(여러 줄 안 이동, 첫·끝 줄에서 기록 호출).
6. 기록은 `repl-history.jsonl`(줄마다 JSON 문자열, Unix 0600, 최근 500개)로 바꿔 여러 줄 목표를 보존한다.
   `xgen> `, `...> `에서 보낸 것만 남기고 승인 응답은 남기지 않는다. 예전 `repl-history.txt`는 읽지 않는다.
7. 종료·panic 시 raw mode와 bracketed paste를 되돌리고 pane을 지운다.
8. 터미널이 아니면(파이프, 테스트) 기존 stdin reader와 plain 출력을 그대로 쓴다.

의존성: `rustyline`을 빼고 `crossterm` 0.29.0(`events`, `bracketed-paste`, `windows`)과 `unicode-width` 0.2.2를 넣었다.
lock의 package 수는 333에서 336이 됐고, Windows의 BSL-1.0 의존성이 사라져 `about.toml` allow-list에서 뺐다.

## 검증

- 단위 테스트 5개: 출력 분리(`\r` 진행 줄과 끝난 줄), 줄 편집기(한글 경계, 단어·줄 삭제, 여러 줄 위아래, 기록),
  표시 폭 기준 줄바꿈과 caret 위치, pane 배치(진행 중·승인 질문), 기록 파일 왕복과 0600.
- tmux 실사용(DeepSeek profile, 비trust, 100x30):
  - 승인 질문이 pane에 뜨고 답이 scrollback에 `질문 + 답`으로 남는다.
  - Run 중 진행 줄이 pane에서 갱신되고 activity 줄은 위로 올라간다. 그동안 입력창에 쓸 수 있다.
  - Run 중 Enter로 넣은 목표가 `queued:`로 보이다가 Run이 끝나자 바로 실행됐다.
  - Run 중 Ctrl+C로 `paused: user_cancelled`가 됐다.
  - Alt+Enter 두 줄 목표가 하나의 목표로 들어갔고, 위 화살표로 두 줄 그대로 다시 불렀다.
  - Ctrl+D 종료 후 shell의 `stty`가 `icanon`으로 돌아왔다.

## 남은 일

- 창 크기를 줄이면 이미 그려진 pane 일부가 scrollback에 남을 수 있다. 다음 그리기에서 덮지만 완전하지 않다.
- 긴 결과 접기, 색 강조 같은 표시 개선은 따로 다룬다.
