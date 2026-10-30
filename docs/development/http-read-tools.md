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

OpenAPI 3의 서버 prefix는 `--base-url`에 넣는다. Swagger 2의 `basePath`는 원본 계약에서 가져와 주소 뒤에 붙이므로 같은 prefix를 두 번 넣지 않는다. 연결은 이름별 불변 파일로 저장하고 `connect`는 기존 연결을 덮어쓰지 않는다(`http_connection_already_exists`).

```bash
xgen tools disconnect --name my-system   # 연결과 OS secret store의 토큰만 삭제, collection은 유지
xgen tools connect --name my-system --base-url https://api.example.com --allow-get --bearer
xgen tools remove --name my-system       # collection, 연결, 저장된 토큰을 함께 삭제
```

- `disconnect`·`remove`는 그 연결이 참조한 OS secret store 항목을 먼저 지우고 연결 파일을 지운다. 토큰 삭제가 실패하면 연결을 남긴 채 `http_credential_store_unavailable`로 멈추므로 같은 명령을 다시 실행하면 된다(이미 없는 항목 삭제는 성공으로 본다). 결과의 `stored_credential`은 `removed`, `none`(환경변수 이름 연결·인증 없음), `unknown`(손상된 연결 파일이라 참조를 알 수 없음)이다. 다른 연결의 토큰은 건드리지 않는다.
- `remove`는 연결을 collection보다 먼저 지운다. 연결이 남은 채 collection만 지워지면, 나중에 같은 이름으로 가져온 다른 API에 이전 주소와 토큰이 붙기 때문이다. 같은 이유로 `import`는 collection 없이 연결만 남은 이름(collection 파일을 손으로 지운 경우)을 `http_connection_without_collection`으로 거절한다. `disconnect` 후 다시 가져온다.
- 연결 저장소가 사용자 전용이 아니거나, 연결 파일이 symlink거나, 저장소를 읽을 수 없으면(권한 오류를 "연결 없음"으로 보지 않는다) 아무것도 지우지 않고 실패한다.
- 연결을 바꾸면 그 연결로 시작한 Run은 재개 시 기존처럼 거절된다.

## HTTP 지원 범위 점검

```bash
xgen tools check-http --name my-system
xgen tools check-http --name my-system --offset 0 --limit 100
```

개별 `describe`와 같은 판정으로 import된 도구의 지원·거절·선택 파라미터 제외를 JSON으로 모은다.
`summary`는 전체 집계이고 `tools`는 이름순 페이지다. 제외 파라미터 수는 지원된 계약에서 실제로 제외한 항목만 센다. 다음 페이지는 반환된 `page.next_offset`을
`--offset`으로 넣는다. 값이 null이면 끝이다. 기본100개·최대500개이며 행 byte 한도에 먼저 닿으면
더 적게 반환할 수 있다. 페이지를 합칠 때 `artifact_digest`와 `policy_digest`가 같은지 확인한다.

연결이 있으면 저장된 GET/POST 조회 지정 정책을 반영한다. 연결이 없으면 기존 describe와 같은
GET 계약 판정이며 `connection_present=false`다. tool·원본 operationId·method·path는
`text`/`omitted` preview이므로 잘린 이름으로 operation 지정을 시도하지 않는다.
제외한 파라미터는 count·complete와 함께 이름·위치·이유를 제공한다.

이 명령은 토큰을 읽거나 API·모델을 호출하지 않는다. `supported`는 **계약 판정**이며 인증·실제 입력값·
API 응답의 검증이나 과업 성공을 뜻하지 않는다. `scope=imported_tools`이므로 import에서 빠진 원본
operation의 수까지 확인한 결과도 아니다. unsupported 도구가 있어도 보고서 생성에 성공하면 exit0이다.
[ADR-0085](../adr/0085-http-read-support-report.md).

## Run별 bearer 주입

```bash
xgen run --workspace . --allow-dir . --allow-read --allow-remote-model-egress \
  --bearer-env-for my-system=USER_API_TOKEN "연결된 API를 조회해줘"
xgen resume RUN_ID --workspace . --allow-dir . --allow-read --allow-remote-model-egress \
  --bearer-env-for my-system=USER_API_TOKEN
```

토큰은 호출 프로세스의 `USER_API_TOKEN`에 호스트가 설정한다. 인자에는 값 대신 **환경변수 이름**만
넣는다. 여러 연결이면 옵션을 반복한다. 연결 파일의 bearer 참조는 바뀌지 않고 이 Run에서만 우선한다.
변수가 없거나 빈 값·잘못된 bearer면 저장 토큰으로 넘어가지 않고 거절한다. process 도구의 safe
환경으로 전달되는 `LANG`·`PATH` 같은 변수는 인증 참조로 사용할 수 없다.

미완료 Run을 재개할 때 원래 참조들을 다시 지정해야 한다. map 누락·변경은 재개를 거절하고, 같은
참조에서 토큰 값만 바꾸는 것은 가능하다. 값은 manifest·Receipt·모델에 남지 않는다. 완료 Run의
재생은 참조 옵션이나 토큰 없이 가능하다. [ADR-0084](../adr/0084-run-bearer-overrides.md).

서버는 사용자별 자식 프로세스 환경으로 주입한다. 공유 프로세스의 전역 환경을 요청마다 바꾸는 방식은
사용자 격리가 아니다. 사용자 인증·대리 권한·Run 접근 제어·상태 저장소 격리는 호출 서버가 담당한다.

## 호스트가 지정한 POST 조회

```bash
xgen tools connect --name my-system --base-url https://api.example.com \
  --allow-get --read-operation searchRecords --read-operation inspectRecords --bearer-env API_TOKEN
```

`--read-operation`은 원본 OpenAPI operationId를 반복 지정한다. `--allow-get` 없이 선택한 GET/POST만
허용할 수도 있다. 지정하지 않은 POST, 유일하지 않거나 없는 ID, PUT/PATCH/DELETE는 거절한다.
이는 호스트의 조회 선언이며 서버의 실제 부작용을 xgen이 추론하거나 보장하는 기능이 아니다.

POST는 OpenAPI `requestBody`의 단일 `application/json`과 원본 schema를 사용한다. read 입력의
선택 필드 `body`에 JSON을 넣고 required·전체 schema를 검증한다. 입력 전체8KiB 한도는 유지한다.
Swagger2 body와 여러 미디어 형식은 아직 지원하지 않는다. 목록은 연결·manifest·execution profile에
고정되어 바꾸면 기존 Run 재개가 거절된다. [ADR-0083](../adr/0083-host-selected-post-reads.md).

## 실행 계약

agent loop에는 연결이 있을 때 `xgen.http/read@2.0.0`이 추가된다. 외부 도구마다 실행용 Definition을 자동 등록하는 기능과는 구분한다. 검색과 describe는 후보를 제공하고, read 프록시는 exact collection·tool·원본 HTTP 계약 digest·입력을 기존 material/policy/executor/verifier 경로에 연결한다.

입력은 `collection`, `tool`, describe의 `http_read.contract_digest`를 복사한 `contractDigest`, `parameters: {path: {...}, query: {...}}`다. 모델은 URL·인증·헤더·HTTP method를 지정하지 못한다. collection snapshot과 연결 설정은 manifest와 execution profile에 고정된다. 새 연결은 이전 Run에 추가하지 않고, 기존 연결이 바뀌면 재개를 거절한다. 인증 값 자체는 manifest에 넣지 않으며 만료된 토큰의 교체는 같은 호스트 인증 참조를 통해 가능하다.

원본 OpenAPI JSON을 압축 artifact에 함께 보존한다. graph-tool-call의 정규화된 `api_contract`만으로는 원래의 HTTP 응답 스키마와 보안 요구를 완전히 복원할 수 없으므로 이를 실행 계약으로 사용하지 않는다. offline worker가 원본 계약을 선택·해제하고 호스트가 입력과 성공 응답을 검증한다. describe는 실행 가능 여부와 거절 코드를 `http_read`에 따로 제공한다. 모델용 describe 2.0.0은 전체 계약 digest만 전달하고 호스트는 별도 원본 계약 경로로 검증한다. 전체 계약 한도는 8 MiB이고 [view·페이지·버전 이행](bounded-tool-describe.md)을 따른다.

호출 결과의 URL·status·body·exact request를 관찰 artifact로 저장한다. Receipt에는 검증 결과와 artifact/output digest를 연결하고, 최종 모델 요청에는 검증을 통과한 tool output을 제공한다. HTTP 성공과 스키마 일치가 데이터의 업무상 진실까지 보증하는 것은 아니다.

## 지원 범위와 한도

- 새로 가져온 **로컬 JSON 문서만으로 구성한 collection**의 GET과 호스트 지정 POST 조회. OpenAPI 3.1/3.0 및 Swagger 2의 아래 부분집합을 지원한다.
- 지원 밖인 선택 파라미터는 operation을 막지 않고 실행 입력에서 제외한다. 모델용 describe의
  `http_read.excluded_parameters`에 이름 preview·위치·고정 이유를 표시하고 count/digest/complete를
  함께 준다. 제외한 이름을 입력에 넣으면 HTTP 호출 전에 거절한다. required와 path 파라미터가
  지원 밖이면 operation 전체를 거절한다. [ADR-0081](../adr/0081-optional-http-parameters.md).
- query 배열은 OpenAPI 3 `style=form, explode=true`(기본값 포함)와 Swagger 2
  `collectionFormat=multi`만 지원한다. JSON 배열을 반복 query key로 직렬화하며 items의 scalar 제약과
  minItems/maxItems/uniqueItems를 검증한다. 빈 선택 배열은 pair를 만들지 않고 빈 required 배열은 거절한다.
  다른 직렬화·object items·path 배열은 선택 제외/필수 거절 대상이다.
  [실행 profile v2와 이전 Run 재개](../adr/0082-array-query-parameters.md).
- scalar path/query: string/integer/number/boolean, simple path·form query, 원본 required·enum·pattern·수치/길이 제약. 값은 URL library로 encode한다. path의 dot segment·slash·역슬래시·percent 입력과 혼합 template는 거절한다.
- 명시적인 숫자 2xx 응답의 `application/json` 스키마. 실제 응답도 해당 Content-Type이어야 한다. 순환 없는 문서 내부 `$ref`를 해제한다.
- 인증 없음 또는 OpenAPI HTTP bearer. OAuth·API key·cookie·header parameter·본문·지원 밖 배열 파라미터·외부/순환 ref·ref sibling·nullable/legacy boolean exclusive bound·dynamic ref는 호출 전에 거절한다. JSON Schema 2020-12에 호환되는 스키마만 실행한다. format은 annotation이며 추가 format 보증은 없다.
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
  --python 3.12 --with graph-tool-call==0.47.0 -- \
  python -m unittest discover -s scripts/tests -p 'test_*tools.py' -v
cargo test -p xgen-cli
cargo clippy -p xgen-cli --all-targets -- -D warnings
```

결과와 남은 개발은 [공동 TODO](tool-integration-todo.md)에서 관리한다. 다음은 원격 명세의 원본 보존·연결 점검 UX와 HTTP 계약 지원 확대이고, 이후 브라우저 관찰·실행과 SEV 연결을 진행한다.
