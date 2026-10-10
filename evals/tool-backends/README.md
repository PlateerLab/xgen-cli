# 검색·터미널 backend 실험

채택 판단과 결과 해석은
[개발 기록](../../docs/development/open-source-tool-backends-2026-10-05.md)을 참고한다.
이 실험은 production 도구를 자동 활성화하지 않는다.

## 검색 비교 재현

DDGS 9.16.0을 별도 Python 환경에 설치하고, OpenSERP v0.8.12 release와 Chrome/Chromium을 준비한다.
아래 path는 각자의 설치 위치로 바꾼다. 외부 검색 서비스에 공개 질의를 전송한다. 모델 API는 호출하지 않는다.

```bash
python3 scripts/experiment-tool-backends.py \
  --ddgs-python /absolute/path/to/venv/bin/python \
  --openserp /absolute/path/to/openserp \
  --browser /absolute/path/to/chrome \
  --output /absolute/path/to/search-report.json
```

POSIX `waitid/WNOWAIT`와 process-group 정리를 사용하는 runner라 지원하지 않는 환경에서는 실행을 거부한다.
네트워크·검색엔진·환경에 따라 결과는 달라진다. 결과 URL 검사는 syntax 검증이며, 해당 URL을
직접 fetch하는 구현이나 SSRF 방어 검증을 포함하지 않는다.

## 기존 process 경로 live 연결

이 명령은 현재 model profile과 secure store를 사용하며 모델 API 비용이 발생한다.
fresh private Run store를 생성한다. `--root`는 존재하지 않는 실험용 경로여야 한다.
approval pause와 resume, durable output·Receipt, 출처 exact match, offline replay를 확인한다.
하나라도 실패하면 보고서를 남기고 nonzero exit로 종료하며 자동 retry하지 않는다.

```bash
python3 scripts/smoke-search-process.py \
  --binary /absolute/path/to/xgen \
  --openserp /absolute/path/to/openserp \
  --root /absolute/path/to/new-private-experiment
```

## Offline PTY 검증

```bash
cargo run -p xgen-adapter-process --example pty_backend_probe --locked
python3 -m unittest discover -s scripts/tests -v
```

Linux에서 현재 executable을 fixture로 실행하며 임시 workspace와 test 소유 process group만 사용한다.
`Child::kill()`의 하위 process 잔존은 의도적으로 관찰하는 실패 사례다. group cancellation 등 검증한 보완
경로가 실패하면 probe 자체가 실패한다. Linux 이외는 `not_validated`이고 production PTY 지원을 뜻하지 않는다.

## Typed web search live 연결

후속 구현인 `xgen.web/search`는 별도 native capability다. generic process 실험과 구분해서 평가한다.
영어·한국어 기존 사례와 별도 SQLite·Rust 사례를 사용하며, 입력 query 일치와 generic process 호출 0개도 검사한다.

```bash
python3 scripts/smoke-web-search.py \
  --binary /absolute/path/to/xgen \
  --openserp /absolute/path/to/openserp \
  --root /absolute/path/to/new-private-typed-experiment
```

[구현·검증 기록](../../docs/development/typed-web-search-2026-10-05.md)을 참고한다.
