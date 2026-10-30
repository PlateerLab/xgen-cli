# ADR-0079: inline pane은 PTY 크기 대신 실제 터미널 화면 크기로 배치한다

- 상태: Accepted
- 기준일: 2026-10-30
- 분류: 범용 TUI 표시 수정. 모델 입력·journal·protocol·승인 계약 변경 없음.
- 후속 결정: ADR-0077의 화면 크기 조회, ADR-0078에서 남긴 빠른 resize 잔상.

## 원인

40×12로 줄인 뒤 100×30으로 키우면 이전 좁은 top border가 새 pane 위에 남았다.
입력 중 draft와 실행 중 queue에서 각각 재현했고, 변경 전 main `ad2a898`에서도 확인했다.

터미널 화면과 PTY의 `TIOCGWINSZ` 크기가 동시에 바뀐다는 가정이 잘못됐다.
tmux는 screen을 즉시 resize하지만 PTY resize 통지는 queue·timer로 늦출 수 있다.
그 사이 periodic draw가 이전 PTY 크기를 읽어 이미 reflow된 화면에 다시 그렸다.
나중에 SIGWINCH가 오면 caret은 중간 draw가 옮긴 위치라, 기존 cursor 기반 복구로는 남은 border를 찾지 못했다.

근거는 격리된 tmux/loopback fixture의 syscall trace와 process를 멈춘 직후의 screen/cursor capture다.
화면은 100×30·cursor row 27인데 PTY 크기는 잠시 40×12였고, 그 사이 draw 이후 cursor row가 9로 바뀌었다.
이는 cursor 응답이 캐시됐다는 초기 추측과 달랐다. tmux 3.4의
[screen resize](https://github.com/tmux/tmux/blob/3.4/window.c#L1006)와
[PTY resize queue/timer](https://github.com/tmux/tmux/blob/3.4/server-client.c#L2099)도 이 순서를 설명한다.

## 결정

1. Unix의 draw·pane 정리는 화면 자체에서 폭·높이를 조회한다. cursor를 숨기고 위치를 저장한 뒤,
   화면 밖의 충분히 큰 좌표로 이동한다. 터미널이 화면 마지막 셀로 제한한 실제 cursor 위치를 CPR로 읽고
   폭·높이로 바꾼다. 원래 위치와 frontend의 cursor 가시성을 복원한 뒤 기존 지우기·배치를 수행한다.
   CPR이 실패해도 원래 위치 복원을 시도하고 기존 오류 종료 경로를 따른다.
2. Windows는 기존 console size API를 유지한다. WinAPI의 화면 밖 좌표 이동은 Unix 터미널처럼
   clipping을 보장하지 않으므로 Unix probe를 적용하지 않는다.
3. 고정 sleep·tmux 환경 감지·size별 보정값·전체 화면/scrollback 삭제를 추가하지 않는다.
   기존 origin·hard-line 폭 기반 pane 지우기와 terminal scrollback 보존을 유지한다.
4. Unix는 redraw마다 CPR 왕복이 하나 늘어난다. 원격 터미널의 latency 비용은 남아 있다.
   실제 화면 크기와 PTY 크기가 일치한다고 가정해 조회를 생략하면 같은 결함이 돌아오므로,
   후속 최적화는 별도 측정과 회귀 검증을 요구한다. 원래 TUI도 시작·출력·resize에 CPR을 사용했다.

## 검증

- 설계: 입력 중 draft, 모델 응답 대기 중 queue의 빠른 축소·확대에서 border 한 세트·시작 출력 한 번을 검사한다.
  수정 전에는 100×30 복원에서 실패했고 수정 후 연속 5회 통과했다. 모델 요청은 실행당 3회다.
- 별도 검증: 53×17·91×26·28×9·80×24를 세 번씩 중간 frame 대기 없이 변경한 뒤 복원한다.
  설계와 다른 크기의 burst에서도 scrollback·입력·승인·완료·history·종료 후 icanon 복원을 확인했다.
- 결과 viewer를 포함한 smoke도 빠른 resize로 통과했다. 전체 보기 이동·승인 Enter 격리·보류 출력·입력 복원을
  유지하며 모델 요청은 5회다. 새 유료 모델 호출이나 provider 재요청은 없다.
- Fast/Linux CI는 기존 정상 속도 smoke에 기본·결과 viewer의 `--rapid-resize` smoke를 추가한다.
- Rust 전체 773 passed·4 ignored, Clippy·fmt·공개 문서·배포 workflow 검사 통과.
  실제 화면 관찰은 Linux/tmux이며 다른 플랫폼의 build/unit 검사와 구분한다.

## 책임

PTY와 screen의 갱신 순서는 tmux의 구현 특성이다. xgen이 두 크기를 같다고 가정하고 그린 것이 표시 결함의 원인이다.
수정 책임은 xgen TUI에 있으며 tmux 자체 변경이나 다른 도구 저장소의 patch는 필요하지 않다.
