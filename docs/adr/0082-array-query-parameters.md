# ADR-0082: query 배열은 scalar items와 반복 키 직렬화만 지원한다

- 상태: Accepted
- 기준일: 2026-10-31
- 분류: 범용 HTTP 조회 확장 2단계. 도메인별 이름이나 판단을 사용하지 않는다.
- 후속 결정: ADR-0081의 파라미터별 지원 판정.

## 결정

1. OpenAPI 3의 query `type: array` 중 `style: form`, `explode: true`만 허용한다. 두 값은
   query/form의 기본값도 인정한다. Swagger 2는 `collectionFormat: multi`만 허용한다.
   모델 입력은 JSON 배열이고 host가 `?states=open&states=closed`처럼 각 값을 별도 key/value로 쓴다.
2. `items`는 명시적인 string/integer/number/boolean schema여야 한다. 기존 scalar enum·pattern·범위와
   배열의 minItems/maxItems/uniqueItems를 원본 전체 JSON Schema로 호출 전에 검증한다.
   항목별 길이·control character 제한과 전체 입력·URL 한도도 유지한다. URL 한도는 pair 추가 중에도 확인한다.
3. explode=false, spaceDelimited/pipeDelimited/deepObject, Swagger csv/ssv/tsv/pipes/default csv,
   object·중첩 배열 items, tuple prefixItems와 path 배열은 지원하지 않는다.
   선택 항목이면 ADR-0081처럼 제외 이유를 표시하고, required/path이면 operation을 거절한다.
4. 빈 배열은 반복할 member가 없으므로 query pair를 만들지 않는다. 선택 배열은 이 표현을 허용하되,
   required 배열이 wire에서 사라지는 것은 호출 전에 required 오류로 거절한다. minItems는 그와 별도로 적용한다.
   빈 문자열 하나를 가진 배열은 빈 배열과 다르며 기존 scalar 규칙을 따른다.
5. HTTP 실행 profile을 `xgen.http.read/v2`로 올리고 binding과 local execution profile이 같은 상수를 사용한다.
   Capability 입력의 기존 path/query JSON object 형식과 `xgen.http/read@1.0.0`은 유지한다.
   이전 HTTP profile의 미완료 Run은 configuration_mismatch/execution_profile로 닫고 새 Run을 요구한다.
   완료된 Run의 offline replay는 추가 API·모델 요청 없이 유지한다.

## 검증

- 설계: assets(OpenAPI 3)와 calendar(Swagger 2)에서 네 scalar items의 enum·pattern·범위 및
  minItems/maxItems/uniqueItems 보존, 지원 밖 serialization의 선택 제외·required 거절을 확인한다.
- host unit은 string·integer·number·boolean 반복 pair, Unicode·예약 문자 encoding, 개수·중복·enum·
  잘못된 items·control character·item/URL 한도와 빈 required 배열을 검사한다.
- 별도 shipment·sensor에서 import→실제 loopback HTTP를 실행하고 repeated query의 값 순서를 검사한다.
  잘못된 개수·중복·enum·숫자 범위는 HTTP 0회로 차단한다. 제외된 serialization 입력도 계속 거절한다.
- 이전 바이너리로 read 승인 대기 Run을 만든 뒤 새 바이너리로 resume하면 HTTP·모델 추가 호출 없이
  execution_profile drift를 거절한다. 이전 바이너리의 완료 Run은 새 바이너리에서 네트워크 없이 재생된다.
- 외부 커머스 API와 실제 모델 품질 비교는 이번 설계·검증에 포함하지 않는다.

## 근거

[OpenAPI Parameter serialization](https://swagger.io/docs/specification/v3_0/serialization/)과
[RFC 6570 §2.3](https://www.rfc-editor.org/rfc/rfc6570#section-2.3)의 composite value expansion을 따른다.
빈 list는 undefined로 취급되므로 required 파라미터의 wire presence를 별도로 보존한다.

로컬 검증: Rust 781 passed·4 ignored, system Python venv+graph-tool-call0.47.0+컴파일 CLI 전체
144개 검사(7 skipped) 통과. 이전 HTTP v1 바이너리를 주입한 두 실제 migration 검사도 실행했다.
Clippy·fmt·공개 문서·workflow 검사 통과. 유료 모델·외부 운영 API 호출은 없다.
