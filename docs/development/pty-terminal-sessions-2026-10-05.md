# PTY terminal session 구현 및 검증

일반 기능으로 `xgen.terminal/start`, `read`, `write`, `terminate`를 기존 host에 연결했다. 계약은 [ADR-0049](../adr/0049-command-scoped-pty-sessions.md)에 있다.

## 사용

Linux에서는 기존 모델 설정으로 `xgen`을 실행하면 PATH에서 발견한 개발 도구 catalog와 PTY 도구가 함께 제공된다. 새 terminal package 설치나 API key 설정은 필요 없다. 모델에 대화형 명령 실행·출력 확인·입력 또는 중단을 요청하면 된다. catalog에 없는 실행 파일은 자동 실행하지 않는다.

Headless 예시:

```bash
xgen run --workspace . --allow-dir . \
  --allow-executable python=/absolute/path/python3 \
  --allow-remote-model-egress --allow-read --allow-execute \
  --max-model-turns 12 \
  'PTY로 대화형 프로그램을 실행하고 필요한 입력을 보낸 뒤 결과를 확인해줘'
```

session은 현재 run/resume 실행 구간에서만 살아 있다. pause나 완료 시 정리되고 다음 호출에서 이어 붙이지 않는다. 지속 실행할 server나 daemon 용도에는 아직 맞지 않는다. macOS·Windows PTY는 별도 작업이며 현재 generic process 도구를 사용한다.

## Live 평가

Linux의 default model profile을 사용한 영어 입력·한국어 입력은 설계 사례다. 별도 검증 사례는 exit code 7을 반환하는 프로그램과 child process를 가진 대기 프로그램의 명시적 종료다. 입력 코드·marker는 평가 fixture에만 있으며 engine에는 없다. 자동 retry 없이 실패한 시도도 보고서에 보존한다.

[초기 결과](../../evals/tool-backends/results/2026-10-05/pty-terminal-initial.json)에서 4/4건을 통과했다. 영어·한국어 입력은 start/write/read 각 1회, 비정상 종료는 start/read 각 1회, process tree 종료는 start 1회/read 2회/terminate 1회였다. 각 effect에 Receipt가 있었고 generic process 호출, Unknown, 중복 effect start는 없었다. 최종 응답은 마지막 read의 state·exitCode·output과 정확히 일치했다. 완료 후 invalid model endpoint로 replay해도 저장된 응답을 반환하고 journal·usage·fixture 실행 횟수가 바뀌지 않았다.

마지막 출력 수집 완료를 구분하는 draining 상태를 추가한 뒤 [중간 검증](../../evals/tool-backends/results/2026-10-05/pty-terminal-draining.json)과 [최종 binary 검증](../../evals/tool-backends/results/2026-10-05/pty-terminal.json)을 각각 수행해 모두 4/4건을 통과했다. 세 cohort 총 12/12건이며 모든 시도를 보존했다. 이전 결과도 최종 evaluator의 엄격한 JSON type 비교로 다시 검사했다. 소수의 fixture 통과는 모든 대화형 프로그램이나 platform의 호환성을 증명하지 않는다.

## 재현

```bash
python3 scripts/smoke-pty-terminal.py \
  --binary target/release/xgen --root /private/new/pty-evaluation
```

Live runner는 실제 model API를 사용한다. 공개 CI에서는 네트워크 없이 PTY fixture와 local mock provider를 검사한다. 공개 결과에는 credentials·workspace host path·private SQLite journal을 포함하지 않는다.

Offline 검증: Rust 전체 654개, Python 44개, npm 14개 테스트 통과. 마지막 snapshot 상태 보완 후 terminal unit 8개를 재검사했다. workspace clippy·format·release build·license notices·public docs 및 release workflow 계약 검사도 통과했다.
