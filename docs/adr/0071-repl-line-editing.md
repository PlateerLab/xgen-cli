# ADR-0071: 대화형 REPL 입력에 줄 편집과 기록을 붙인다

- 상태: Accepted
- 기준일: 2026-10-27
- 변경 분류: 범용 REPL 입력. protocol·journal·planning context 변경 없음.

## 문제

대화형 `xgen`은 stdin을 줄 단위로 읽었다. 터미널의 cooked mode가 Backspace만 처리했으므로 왼쪽 화살표는
`^[[D`로 입력에 섞였다. 위 화살표로 이전 목표를 다시 부를 수도 없었다. Claude Code·Codex는 줄 편집과 입력 기록을
기본으로 준다.

## 결정

stdin과 stdout이 모두 터미널이면 입력 thread가 `rustyline` 18.0.1 editor로 한 줄씩 읽는다.

- 의존성은 `default-features = false, features = ["with-file-history"]`다. 사용자 정의 keymap, 완성, 색 강조는
  넣지 않았다.
- 기존 `InterruptibleInput` 구조는 그대로다. REPL이 다음 줄을 요청할 때만 editor가 읽는다. 모델 호출 중 Ctrl+C는
  기존 SIGINT 취소 경로가 처리하고, 입력 중 Ctrl+C는 그 줄만 지운다(`Interrupted`). Ctrl+D는 입력 끝이다.
- prompt는 REPL이 이미 stdout에 쓴다. `PromptTrackingOutput`이 마지막 줄바꿈 뒤 텍스트를 기억하고, editor는 그
  텍스트에서 CSI escape와 제어 문자를 뺀 값을 prompt로 다시 그린다. 256자를 넘으면 prompt가 아니라고 보고 버린다.
- 기록 파일은 Run과 같은 state root의 `repl-history.txt`이고 Unix에서 0600이다. 목표와 명령(`xgen> `, `...> `
  prompt에서 받은 줄)만 저장한다. `[y/a/N]` 승인 응답은 저장하지 않는다.
- 터미널이 아니면(파이프, 테스트) 기존 stdin reader와 plain 출력을 그대로 쓴다.
- Windows에서 rustyline이 끌어오는 `clipboard-win`·`error-code`가 BSL-1.0이다. BSL-1.0은 바이너리 배포 시 고지
  의무도 없는 OSI 승인 라이선스라 `about.toml` allow-list에 더하고 `THIRD_PARTY_LICENSES.txt`를 다시 생성했다.

## 검증

- 단위 테스트: styled prompt에서 escape를 뺀 `xgen> `, 승인 prompt 교체, 256자 초과 폐기.
- tmux 실사용(DeepSeek profile, 비trust): 왼쪽 화살표·Backspace 편집, 입력 중 Ctrl+C, 위 화살표 기록 호출, 승인
  prompt가 자기 줄에 그려지는지, 기록 파일에 목표 1줄만 남고 `a`·`y` 응답은 없는지 확인했다.
- 권한 775인 새 `XGEN_STATE_HOME`에서는 main 바이너리도 `internal_safety_failure`로 멈춘다. 이번 변경과 무관한
  기존 fail-closed 동작이다. 원인을 알려 주는 메시지는 후속 과제로 남긴다.

## 남은 일

- 전체 화면 TUI(입력 영역 고정, 진행 영역 스크롤)는 별도 결정으로 다룬다.
