# ADR-0077: inline TUI를 실제 화면 하단에 고정하고 resize 뒤 pane을 다시 배치한다

- 상태: Accepted
- 기준일: 2026-10-30
- 변경 분류: 범용 터미널 표시·입력과 HTTP 의존성 갱신. protocol·journal·모델 입력 계약 변경 없음.
- 후속 결정: ADR-0074의 pane 배치와 지우기 방식
- 화면 크기 조회 방식은 [ADR-0079](0079-terminal-screen-size.md)에서 보완했다.

## 문제

기존 frontend는 pane 안의 caret row만 기억하고 그만큼 커서를 위로 움직인 뒤 `CSI J`로 아래를 지웠다.
두 문제가 겹쳤다.

1. 출력이 적으면 pane이 실제 하단이 아니라 출력 바로 아래에 있었다. 폭 축소 시 터미널이 줄을 reflow하면서
   pane이 scrollback으로 밀려났다. 이전 caret row로는 바뀐 물리 행 수를 복구하지 못했다.
2. pane이 화면 첫 줄로 올라간 뒤 `CSI J`를 반복하면 tmux가 지워진 화면을 scrollback에 보존했다.
   입력하지 않아도 매 redraw마다 pane 복사본이 쌓였다.

추가로 layout이 최소 폭 20을 가정했고 대기열 행 수도 제한하지 않아 좁거나 낮은 창에서 pane이 화면을 넘었다.
100×30 → 40×12 및 별도의 낮은 창·대기열 layout에서 재현했다.

## 결정

1. pane의 top을 `terminal height - pane rows`로 정하고 절대 좌표에 그린다. 완료 출력은 pane 앞에 쓰고,
   자리가 모자라면 필요한 행만 scroll up한다. alternate screen이나 전체 scrollback 삭제는 쓰지 않는다.
2. 그린 pane의 terminal size, origin, caret, hard-line 표시 폭을 기억한다. 크기가 바뀌면 현재 cursor 위치를
   조회하고 이전 행의 새 물리 줄바꿈 수를 고려해 지울 시작점을 복구한다. 새 출력 뒤 cursor 위치도 조회한다.
   크기·출력이 바뀌지 않은 frame에서는 저장한 origin을 사용해 cursor 조회 왕복을 피한다.
3. pane 지우기는 `CurrentLine`만 사용한다. 이전 pane이 차지한 물리 행만 화면 높이 안에서 지우고 cursor를
   복원한다. `CSI J`·전체 화면·scrollback clear를 사용하지 않는다.
4. keyboard와 resize event를 한 tick에 최대 256개 모아 반영한 뒤 그린다. 이벤트가 계속 들어와도 출력·종료
   channel을 다시 확인한다. 입력과 queue·승인 상태의 논리는 그대로다.
5. 실제 폭·높이 안에서 status, 입력, footer, queue를 배치한다. 입력과 승인 status를 우선하고 대기열 표시를
   제한하며 생략한 수를 알린다. 폭 8 미만 또는 높이 4 미만은 테두리·footer 없는 compact layout으로 표시한다.
   숨긴 queue 항목과 입력 원문은 삭제하지 않는다. 0-column 문자열 자르기는 빈 문자열을 반환한다.

## 실행 중 resize와 HTTP socket 읽기

실제 TUI smoke에서 모델 응답을 기다리는 동안 resize하면 기존 `ureq` 3.4.0의 요청이
`model_call_unknown.transport_unavailable`로 끊겼다. 같은 smoke에서 실행 중 resize만 빼면 읽기·대기열 실행까지
진행했고, resize를 넣은 독립 실행들에서는 같은 연결 중단이 반복됐다.

`ureq` 3.4의 TCP 읽기는 timeout이 설정된 socket read의 `Interrupted`를 그대로 오류로 반환했다.
3.5.0에는 timeout 예산을 유지하며 `EINTR`에 끊긴 socket 읽기를 이어가는 공식 수정(#1205)이 들어 있다.
따라서 자체 transport나 요청 재전송을 추가하지 않고 workspace의 `ureq`를 3.5.0으로 갱신했다.
Lock에는 rustls·ureq-proto·webpki-roots의 연계 갱신도 포함된다. 기존 Unknown·취소·deadline 계약을 유지한다.

이 문제의 원본 수정은 `ureq` 책임이고 xgen은 수정 버전을 선택·회귀 검증한다.

## 검증

- TUI unit 8개: 기존 5개와 새 layout 경계, 지우기 command, resize 중 draft·queue·승인 보존 검사다.
  layout은 폭 1..100 × 높이 1..24 × 승인/취소 상태 4,800개 조합을 검사한다.
- 실제 tmux 3.4/Linux: 100×30, 40×12, 100×12, 80×24의 축소·확대를 반복해 pane 테두리가 한 세트이고
  시작 출력이 한 번만 남는지 확인했다. 별도 수동 관찰에서 30×8도 확인했다.
- `python3 scripts/smoke-inline-tui.py --binary target/debug/xgen`: 새 설정·state·tmux socket과 loopback
  provider만 사용한다. 유료 LLM이나 사용자 credential을 사용하지 않는다. 실행 중 resize·대기열, 승인 답변
  보존·읽기 완료·대기열 실행, 설계와 다른 55×18의 완료 후 resize·history, 종료 후 `icanon` 복원을 확인한다.
  예상 model call 3개 외 재요청이 없는지도 확인한다. 실패/성공 capture와 phase 기록은 출력된 임시 폴더에 남긴다.
  `Fast / Linux` CI에서도 같은 smoke를 실행해 이후 PR의 실제 TUI 회귀를 검사한다.
- 실제 화면 관찰은 Linux/tmux 범위다. 다른 OS의 native terminal 화면 확인과 CI의 build/unit 검증을 구분한다.
- 기존 binary로 같은 tmux smoke를 실행하면 첫 40×12 축소에서 stale-pane assertion이 실패했다.
  변경 binary는 전체 흐름을 통과했다. 전체 Rust 회귀 766개(4 ignored), Clippy·fmt·protocol·문서·배포 workflow·
  license notice 검사를 통과했다.

코드 `e751aef`의 [전체 CI 7개 job](https://github.com/PlateerLab/xgen-cli/actions/runs/41048809159)이
통과했다. 5개 플랫폼 release·설치 검사를 포함한다. CI·smoke harness 보완 `02bd243`의
[Linux PR 검사](https://github.com/PlateerLab/xgen-cli/actions/runs/41049049910)도 통과했고, 실제 tmux smoke는 3초에 완료됐다.
Intel macOS job은 기존 CI 정책대로 테스트를 생략하고 macOS 테스트는 Apple Silicon에서 실행한다.

## 참고

- [ureq 3.5.0 changelog의 EINTR 수정](https://github.com/algesten/ureq/blob/main/CHANGELOG.md#350)
- [inline viewport의 clear와 scrollback 동작 사례](https://github.com/ratatui/ratatui/pull/2670)
