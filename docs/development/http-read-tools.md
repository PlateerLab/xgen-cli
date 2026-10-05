# 연결된 API의 HTTP 조회

2026-10-06. 일반 기능이며 특정 시스템의 업무 판단을 엔진에 넣지 않는다.

## 사용

로컬 JSON OpenAPI 명세를 가져와 시스템을 한 번 연결한다. API 주소·인증은 호스트가 보관하고 도구별 JSON 설정은 만들지 않는다.

```bash
xgen tools import --name my-system --source ./openapi.json
xgen tools connect --name my-system --base-url https://api.example.com \
  --allow-get --bearer
xgen
```

`xgen`의 입력창에서 자연어로 조회를 요청하면 된다.

`--bearer`는 숨김 입력으로 받아 기존 OS secret store에 저장한다. 인증 없는 시스템은 빼면 된다. 자동 실행 환경에서는 `--bearer-env ENV_NAME`을 사용하면 환경변수 **이름**만 연결에 저장하고 실제 토큰은 호출 시 읽는다. `--token-stdin`도 OS secret store에 저장한다. 세 옵션은 함께 사용할 수 없다. 키 값은 채팅·명령 인자에 쓰지 않는다. OS secret store가 잠겨 있거나 사용할 수 없으면 저장이 실패한다.

`--allow-get`은 연결한 시스템의 GET을 조회로 사용할 수 있다는 사용자의 명시적인 지정이다. HTTP method나 도구 설명 자체로 부작용이 없다고 판단하지 않는다. Run마다 기존 read 승인을 거치며 자동 승인 실행은 다음처럼 한다. 모델 프로필은 별도로 기존 설정을 재사용한다.

```bash
xgen run --workspace . --allow-dir . --allow-read \
  --allow-remote-model-egress "연결한 시스템의 정보를 조회해줘"
```

OpenAPI 3의 서버 prefix는 `--base-url`에 넣는다. Swagger 2의 `basePath`는 원본 계약에서 가져와 주소 뒤에 붙이므로 같은 prefix를 두 번 넣지 않는다. 연결은 이름별 불변 파일로 저장한다. 변경하려면 새 collection 이름으로 가져와 연결한다. 연결 수정·삭제 UI는 후속이다.

## 실행 계약

agent loop에는 연결이 있을 때 `xgen.http/read@1.0.0`이 추가된다. 외부 도구마다 실행용 Definition을 자동 등록하는 기능과는 구분한다. 검색과 describe는 후보를 제공하고, read 프록시는 exact collection·tool·원본 HTTP 계약 digest·입력을 기존 material/policy/executor/verifier 경로에 연결한다.

입력은 `collection`, `tool`, describe의 `http_read.contract_digest`를 복사한 `contractDigest`, `parameters: {path: {...}, query: {...}}`다. 모델은 URL·인증·헤더·HTTP method를 지정하지 못한다. collection snapshot과 연결 설정은 manifest와 execution profile에 고정된다. 새 연결은 이전 Run에 추가하지 않고, 기존 연결이 바뀌면 재개를 거절한다. 인증 값 자체는 manifest에 넣지 않으며 만료된 토큰의 교체는 같은 호스트 인증 참조를 통해 가능하다.

원본 OpenAPI JSON을 압축 artifact에 함께 보존한다. graph-tool-call의 정규화된 `api_contract`만으로는 원래의 HTTP 응답 스키마와 보안 요구를 완전히 복원할 수 없으므로 이를 실행 계약으로 사용하지 않는다. offline worker가 원본 계약을 선택·해제하고 호스트가 입력과 성공 응답을 검증한다. describe는 실행 가능 여부와 거절 코드를 `http_read`에 따로 제공한다.

호출 결과의 URL·status·body·exact request를 관찰 artifact로 저장한다. Receipt에는 검증 결과와 artifact/output digest를 연결하고, 최종 모델 요청에는 검증을 통과한 tool output을 제공한다. HTTP 성공과 스키마 일치가 데이터의 업무상 진실까지 보증하는 것은 아니다.

## 지원 범위와 한도

- 새로 가져온 **로컬 JSON 문서만으로 구성한 collection**의 GET. OpenAPI 3.1/3.0 및 Swagger 2의 아래 부분집합을 지원한다.
- scalar path/query: string/integer/number/boolean, simple path·form query, 원본 required·enum·pattern·수치/길이 제약. 값은 URL library로 encode한다. path의 dot segment·slash·역슬래시·percent 입력과 혼합 template는 거절한다.
- 명시적인 숫자 2xx 응답의 `application/json` 스키마. 실제 응답도 해당 Content-Type이어야 한다. 순환 없는 문서 내부 `$ref`를 해제한다.
- 인증 없음 또는 OpenAPI HTTP bearer. OAuth·API key·cookie·header parameter·본문·배열 파라미터·외부/순환 ref·ref sibling·nullable/legacy boolean exclusive bound·dynamic ref는 호출 전에 거절한다. JSON Schema 2020-12에 호환되는 스키마만 실행한다. format은 annotation이며 추가 format 보증은 없다.
- 기존 collection과 URL/Swagger UI로 직접 수집한 collection에는 실행에 필요한 원본이 없으므로 검색은 가능하지만 HTTP 실행은 거절한다. 실행하려면 로컬 JSON으로 새 이름에 다시 가져온다. 원격 수집 원본 보존은 graph-tool-call 연계 후속이다.
- HTTPS, 또는 테스트용 숫자 loopback HTTP만 연결. redirect·환경 proxy·애플리케이션 재시도 없음. 응답 헤더 16 KiB, body 32 KiB, 전체 관찰 64 KiB, 입력·URL 각 8 KiB, HTTP timeout 30초. worker 계약 조회는 기존 offline 120초 한도다.
- 인증은 호스트에서만 추가한다. bearer 값이 본문에 그대로 또는 JSON escape 해제 후 포함되면 본문을 버린다. 인증값·오류 원문·응답 헤더는 worker나 모델 컨텍스트·Receipt에 넣지 않는다. 일반 업무 데이터 전체에 대한 redaction 기능은 아니다.
- HTTP 실패·schema 불일치·본문 한도 초과는 본문 없는 실패 Receipt로 남는다. 실행 중 프로세스가 죽으면 기존 EffectUnknown 복구로 중복 호출을 차단한다. sync 호출의 즉시 취소는 후속이며 현재 HTTP timeout까지 기다릴 수 있다.
- 현재 개발 버전의 `uv` 의존은 유지한다. 별도 MCP 서버는 필요하지 않다.

## 검증과 한계

설계에 사용한 assets/calendar fixture와 별도로 shipment(OpenAPI 3.1·bearer·문서 내부 ref)와 sensor(Swagger 2·basePath·정수 query·배열 응답) fixture를 사용한다. 성공 조건은 동일 엔진에서 검색 → describe → 정확한 GET 1회 → status/schema 검증 → 실제 body에 근거한 최종 응답·완료 재생이다. 거절 조건은 승인/입력/계약/연결 drift에서 GET 0회, HTTP 실패에서 실패 Receipt, 실행 중 kill 이후 자동 재호출 0회다.

실제 graph-tool-call 0.46.0, 컴파일된 CLI, 실제 loopback HTTP API 서버로 검증한다. 모델 endpoint는 결정적 proposal fixture다. 실제 LLM의 도구 선택·답변 품질, 사내 live API, OS keyring backend의 실제 저장/조회는 이번 통합 시험 범위가 아니다. fixture 모델 요청은 성공 사례당 4회이고 실제 조회는 1회, 완료 재생은 추가 호출 0회다. 유료 LLM 호출·과금은 없다.

CLI Rust 178개 통과·live 검사 3개 ignored, protocol 13개·Python 도구 검사 29개(새 HTTP 검사 8개 포함)·Clippy·fmt·공개 문서 검사가 통과했다. release 바이너리에서도 두 fixture의 조회·최종 응답·완료 재생을 재확인했다.

```bash
cargo build -p xgen-cli
XGEN_TOOL_TEST_BINARY="$PWD/target/debug/xgen" \
  uv run --no-project --no-config --no-env-file --isolated \
  --python 3.12 --with graph-tool-call==0.46.0 -- \
  python -m unittest discover -s scripts/tests -p 'test_*tools.py' -v
cargo test -p xgen-cli
cargo clippy -p xgen-cli --all-targets -- -D warnings
```

결과와 남은 개발은 [공동 TODO](tool-integration-todo.md)에서 관리한다. 다음은 원격 명세의 원본 보존·연결 점검 UX와 HTTP 계약 지원 확대이고, 이후 브라우저 관찰·실행과 SEV 연결을 진행한다.
