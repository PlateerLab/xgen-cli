# ADR-0085: import된 도구의 HTTP 조회 지원 보고서

- 날짜: 2026-10-31
- 상태: 채택, 구현·로컬 검증 완료
- 범위: 일반 CLI 진단. API 호출 권한이나 실행 계약은 추가하지 않는다.

## 결정

`xgen tools check-http --name COLLECTION [--offset N] [--limit N]`을 추가한다.
CLI의 기존 연결 정책 로딩 경로를 재사용하고 worker의 기존 http_read_contract를 각 도구에 적용한다.
개별 describe·실행 계약과 다른 지원 판정기를 만들지 않는다. GET 지정과 read_operations 목록을
반영하되 인증값·OS secret store를 읽거나 API·모델을 호출하지 않는다.

전체 summary에는 import된 도구 수, supported/unsupported 수, 거절 이유별 operation 수,
제외한 선택 파라미터 수·이유별 개수를 제공한다. unsupported가 있어도 보고서 생성 성공이면 exit0이다.
저장소·snapshot·페이지 요청 오류는 기존 CLI 오류 경로로 실패한다.

행에는 tool·원본 operationId·method·path, 지원 여부, 지원 계약 digest 또는 거절 코드,
선택 파라미터 제외 이름·위치·이유를 제공한다. 원본 operationId는 보존된 명세에서 조회하며
정규화된 도구 이름을 operationId로 추측하지 않는다. 긴 문자열은 기존 preview(text/omitted) 형식으로
명시적으로 자른다. schema·응답 예시·인증 참조·토큰 값은 보고서에 넣지 않는다.

scope는 imported_tools다. graph-tool-call import에서 누락된 원본 operation까지 센 결과가 아니다.
연결이 없을 때는 기존 describe와 같은 GET 계약 판정이며 실제 호출 승인을 뜻하지 않는다.
connection_present·credentials_checked=false·network_checked=false·execution_enabled=false로
진단의 경계를 명시한다. 지원 판정은 credential 유효성·입력값 검증·API 건강 상태·업무 성공 보장이 아니다.

## 크기와 일관성

도구 이름으로 정렬하며 offset/limit을 제공한다(기본100개, 최대500개). 한 페이지의 행 합계는1MiB,
제외 항목은 도구당32개·이름256 bytes로 제한한다. 도구 이름/경로1024 bytes, operationId128 bytes,
method16 bytes의 preview를 사용한다. 제외 목록의 count/complete를 함께 보낸다.

summary는 페이지 밖 도구까지 모두 집계한다. next_offset은 실제 반환한 행 다음을 가리키므로
행 개수 제한보다 byte 제한에 먼저 닿아도 건너뛰지 않는다. 마지막 offset==total 요청은 빈 페이지와
전체 summary를 반환한다. 페이지를 합치는 호출자는 artifact_digest와 policy_digest가 같은지 확인한다.
policy_digest는 인증 참조가 없는 GET/operation 지정 map 또는 미연결 null만 해시한다.

이 명령은 모델 Capability가 아니므로 Capability 버전·Run manifest·execution profile을 바꾸지 않는다.

## 검증

assets(OpenAPI3)·calendar(Swagger2) 설계 fixture에서 개별 classifier와 summary·페이지 결과를
대조한다. POST 지정 전후·선택 파라미터 제외·빈 목록·잘못된 페이지·마지막 빈 페이지·긴 제외 이름과
1MiB 페이지 경계를 검증한다. shipment/sensor 보류 fixture의 실제 import→connect→check-http를
컴파일 CLI와 pinned backend로 검증한다. bearer 환경변수가 없어도 점검되며 API/모델 호출은0회다.

실제 외부 명세의 실행 가능 과업 수, LLM 완료율, API 응답·인증의 유효성은 측정하지 않는다.

Rust784개(4 ignored), pinned backend+컴파일 CLI Python159개(7 skipped), Clippy·fmt·protocol·문서·workflow 검사를 통과했다.
