# ADR-0070: 진행 중에 끝난 Step의 대상과 결과를 한 줄씩 보여 준다

- 상태: Accepted
- 기준일: 2026-10-27
- 변경 분류: 범용 REPL 표시. protocol·journal·planning context 변경 없음.

## 문제

대화형 `xgen`은 Run이 끝날 때까지 `Reading… · 12s · 3 committed` 한 줄만 보였다. 어떤 파일을 읽고 어떤 명령을
실행하는지는 Run이 끝난 뒤 `Changes` 블록(ADR-0057)에서 쓰기와 실행만 볼 수 있었다. 읽기·검색은 끝까지 보이지
않았다. 40~100단계짜리 작업에서 사용자는 모델이 같은 파일을 반복해 읽는지, 다른 길로 가는지 알 수 없었다.
Claude Code·Codex는 도구 호출을 하나씩 화면에 남긴다.

## 결정

REPL은 Step이 끝날 때마다 그 Step이 한 일을 한 줄로 남긴다.

```
  searched "likes" in . (6 matches)
  read devhub/types.py
  patched devhub/types.py (+1 -1)
  ran bash -c .venv/bin/pytest -q tests/test_types.py (exit 0)
  read gone.md failed: not_found
```

- 출처는 ADR-0057과 같은 검증된 기록이다. Receipt, 승인된 material recipe의 정규화 인자, receipt가 끝난 tool
  output, Step의 `failureCode`를 쓴다. 모델의 objective 문장은 쓰지 않는다.
- `DriverProgress`는 그대로 경로·인자 없는 고정 이벤트다(ADR-0033). REPL host가 이벤트를 받을 때 run journal을
  read-only로 열어, 아직 보고하지 않은 Receipt만 읽는다. 실행 Step의 Receipt는 검증 뒤에 붙으므로, effect commit
  말고 검증 commit, 다음 model call, 완료 때도 확인한다.
- resume는 그때까지의 Receipt 수부터 센다. 이미 보고한 Step을 다시 찍지 않는다.
- 인자를 복원하지 못한 Step은 capability 이름만 보인다. 표시에 실패해도 Run은 계속된다.
- 모든 문자열은 제어 문자를 escape하고, 명령은 120자로 자른다. Run 끝의 `Changes` 블록은 그대로다.
- `--debug`의 `progress:` 줄과 pipe 입력 모드에도 같은 줄이 찍힌다.

## 확인

- e2e: README 읽기와 `git --version` 실행이 각각 `read README.md`, `ran git --version (exit 0)` 한 번씩 찍힌다.
- 단위: 검색·patch·실행·실패 Step의 표시 문자열.
- tmux 실사용(deepseek flash, --trust): 검색, 읽기, patch, pytest 실행이 끝나는 대로 한 줄씩 쌓이고, 아래 live
  line이 계속 다시 그려진다.
