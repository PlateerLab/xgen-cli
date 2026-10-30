# ADR-0081: 지원 밖인 선택 HTTP 파라미터는 실행 가능한 부분집합에서 제외한다

- 상태: Accepted
- 기준일: 2026-10-31
- 분류: 범용 HTTP 조회·모델용 describe 확장. 특정 시스템의 이름·파라미터·업무 규칙을 판정에 사용하지 않는다.
- 후속 결정: [HTTP 조회 계약](../development/http-read-tools.md)과
  [제한된 describe](../development/bounded-tool-describe.md)의 파라미터 투영.

## 결정

1. path-level과 operation-level 파라미터를 `(in, name)`으로 병합해 operation override를 먼저 적용한다.
   각 유효 파라미터를 기존 scalar·location·직렬화·schema 지원 규칙으로 따로 판정한다.
2. 지원 밖인 선택 파라미터만 실행 계약의 `parameters`에서 뺀다. 계약에 `excluded_parameters`를
   추가하고 원본 name·location·고정 reason code를 기록한다. required 파라미터와 path 파라미터는
   지원 밖이면 operation 전체를 거절한다. path는 잘못된 `required:false`가 있어도 생략하지 않는다.
3. 파라미터 identity와 required 여부를 확인할 수 없는 참조, 없는 로컬 참조, 잘못된 구조,
   응답·보안·method·GET body 계약 오류를 선택 파라미터처럼 무시하지 않는다.
   선택 파라미터 schema의 지원 밖 참조·dialect·깊이 제한은 제외 이유로 기록할 수 있다.
   참조 해제는 selected path/parameter object 경계와 schema 경계를 분리해 다른 operation이나
   제외할 선택 schema 때문에 지원 가능한 부분집합을 먼저 잃지 않도록 한다.
4. 모델용 `describe_view`에는 포함한 파라미터만 기존 `tool.parameters` 페이지로 표시한다.
   `http_read.excluded_parameters`에는 이름 preview·location·reason을, 별도 count·digest·complete에
   전체 목록 수·digest와 표시의 완전성을 기록한다. 최대 32개/8 KiB, 이름 preview 1 KiB로 제한하고
   기존 48 KiB describe 한도를 유지한다. 제외된 이름은 실행 가능한 필터가 아니다.
5. 실행은 포함한 파라미터만 허용하는 기존 host `request_url` 경계를 사용한다. 제외한 파라미터를
   path/query 입력에 넣으면 HTTP 요청 전에 unknown parameter로 거절한다. 모델 표시만 숨기지 않는다.
6. 제외 목록도 전체 실행 계약 digest에 넣는다. 원본 전체 source digest도 유지한다.
   제외가 없는 기존 계약에는 빈 목록을 추가하지 않아 기존 supported 계약의 canonical 값은 유지한다.
   이번 단계에서는 배열 직렬화·POST·Run bearer override를 추가하지 않는다.

## 검증

- 설계: OpenAPI 3 assets와 Swagger 2 calendar의 선택 배열·header 제외, scalar 유지,
  required 배열/header·path 거절, override 순서, 선택 serialization/dialect/ref 제외를 검사한다.
- 별도 shipment·sensor의 실제 import→모델 describe→GET→완료에서 선택 배열이 모델 입력 목록에서
  빠지고 제외 이유가 보이는지 확인한다. 제외한 배열을 실행 입력에 다시 넣으면 API 호출은 0회다.
- 이름이 큰 제외 파라미터 100개에서도 describe 한도와 불완전 표시를 유지하며 목록 변경이 digest를 바꾼다.
- 기존 shipment·sensor 계약은 변경 전 main worker와 canonical 값이 같음을 확인했다.
- 기존 HTTP 실패 fixture는 ADR-0052 이전의 즉시 종료를 기대해 main과 같은 계약에서도 실패했다.
  현재 read 실패 복구 계약에 맞춰 모델이 실패 사실을 보고하게 고쳤다. 302·500·잘못된 JSON/schema·
  크기 초과·credential echo 7종에서 failed Receipt·모델에 실패 status만 전달·toolOutputs 없음·
  API 1회·재호출 없음·secret 비노출을 검사한다. 대형 응답 schema 검증 fixture도 같은 방식으로
  failed Receipt를 직접 확인하도록 보완했다. 엔진의 실패 처리 계약은 바꾸지 않았다.

## 범위와 근거

외부 커머스 API는 통합 사례이며 이 설계·검증에는 사용하지 않는다. 그 API에서 실행 가능한 operation 수나
과업 성공률은 아직 측정하지 않았다. 외부 연동 담당자는 로컬 OpenAPI 생성기·endpoint·인증·비교 실행기를
준비하고, xgen은 명세 기반 실행 가능성·직렬화·권한·기록·복구를 맡는다.

[OpenAPI 3.1 Parameter Object](https://spec.openapis.org/oas/v3.1.0.html#parameter-object)의 location·required와
operation의 파라미터 override 규칙을 따른다. 선택 파라미터의 제외는 xgen의 제한된 실행 subset 정책이다.

로컬 회귀: Rust 778 passed·4 ignored, system Python+graph-tool-call 0.47.0+컴파일 CLI 전체 139개 검사
(7 skipped), 그중 HTTP 관련 18개 모두 실행·통과. Clippy·fmt·protocol·문서·배포 workflow 검사 통과.
UV standalone Python 3.12에는 pidfd_open이 없어 기존 평가 process-cleanup 검사가 실패하므로,
해당 기능이 있는 system Python의 별도 venv로 graph-tool-call을 고정해 전체 검사를 실행했다.
