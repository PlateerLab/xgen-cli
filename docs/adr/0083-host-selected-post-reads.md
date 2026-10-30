# ADR-0083: 호스트가 지정한 operation의 POST 조회

- 날짜: 2026-10-31
- 상태: 채택, 구현·로컬 검증 완료
- 범위: 일반 OpenAPI 연결 기능. 외부 통합의 operation 수나 업무 효과를 엔진이 판단하지 않는다.

## 결정

`tools connect --read-operation OPERATION_ID`를 반복해 조회 operation을 지정한다.
`--allow-get`은 기존의 GET 전체 지정이고, 둘 중 하나가 필요하다. 선택 목록은 정렬·중복 제거하며
최대32개, 이름128 bytes, 연결 파일4096 bytes를 넘으면 저장하지 않는다.

원본 source에서 ID가 유일하고 method가 GET/POST인 것만 지정할 수 있다. PUT/PATCH/DELETE는
지정해도 거절한다. 설명이나 HTTP method만으로 조회 효과를 추정하지 않는다. 사용자가 조회로
지정한 operation은 기존 ReadOnly 권한·Receipt·검증 경로를 통과한다.

POST 본문은 OpenAPI requestBody의 application/json 단일 미디어 형식만 지원한다. 내부 ref를
해제한 원본 schema, required를 보존하고 native host가 전체 schema로 검증한다. Swagger2 body,
여러 미디어 형식은 지원하지 않는다. 본문 없는 POST는 원본에 requestBody가 없을 때만 가능하다.
정의하지 않은 본문, 필수 본문 누락, schema 위반은 HTTP 전에 거절한다. 입력 전체8KiB 한도는 유지한다.

`xgen.http/read@2.0.0`에 선택 입력 `body`를 추가한다. method는 모델 입력이 아니라 원본 계약에서
결정하고 결과에서도 검증한다. 모델 describe에 본문 required와 bounded schema view를 제공한다.
전체 schema는 호스트에만 있으며 큰 schema의 view 생략을 검증 생략으로 취급하지 않는다.

연결의 지정 목록은 manifest·execution profile에 포함된다. 목록 변경은 기존 `http_connections`
drift 규칙으로 재개를 막는다. 새 실행 profile은 `xgen.http.read/v3`이다. 이전 미완료 Run은 새
profile로 계속할 수 없고, 완료 Run의 offline replay는 유지한다. 인증 참조·고정 origin·redirect 금지·
응답 schema 검증·credential echo 차단은 기존과 같다.

## 검증

설계 assets와 별도 shipment 원본의 POST 승인·본문 계약을 검증한다. 실제 loopback API/컴파일 CLI로
shipment와 measurement의 POST JSON wire·원본 body 제약·미지정 POST 0회 호출·목록 drift를 검증한다.
다른 method와 알 수 없는 ID는 연결 저장 전에 거절하고, 이전 GET profile의 미완료 거절·완료 replay도
기존 fixture로 검증한다. 결정적 모델 endpoint를 쓰며 실제 LLM 품질이나 외부 API coverage는 측정하지 않는다.

## 근거

- [OpenAPI Operation Object](https://spec.openapis.org/oas/v3.1.0.html#operation-object)
- [OpenAPI Request Body Object](https://spec.openapis.org/oas/v3.1.0.html#request-body-object)
- ADR-0081, ADR-0082의 파라미터 정책을 POST query에도 동일하게 적용한다.
- 승인 있는 쓰기와 Idempotency-Key 정책은 별도 설계로 남긴다.

Rust 전체782개(4 ignored), pinned backend+컴파일 CLI의 Python148개(7 skipped), Clippy·fmt·protocol·문서·workflow 검사를 통과했다.
