# ADR-0084: Run별 HTTP bearer 인증 참조

- 날짜: 2026-10-31
- 상태: 채택, 구현·로컬 검증 완료
- 범위: 일반 headless HTTP 인증 기능. 특정 서비스의 사용자·권한 규칙은 포함하지 않는다.

## 결정

`run`/`resume`에 반복 가능한 `--bearer-env-for CONNECTION=ENV_NAME`을 추가한다. 연결별로 해당
호출의 bearer 환경변수 참조를 선택하며, 저장된 연결·인증 참조·OS secret store는 바꾸지 않는다.
CLI 외부 호스트도 LocalRunRequest/LocalResumeRequest의 bearer_env_for map으로 참조를 전달한다.
대화형 REPL의 기본 연결 인증 방식은 유지한다.

override가 있으면 저장된 인증보다 먼저 사용한다. 지정한 환경변수가 없거나 빈 값·잘못된 bearer면
실행 전 거절하며 저장 토큰으로 fallback하지 않는다. 없는 연결·중복 CLI 지정·잘못된 이름도 거절한다.
이름만 허용하고 실제 bearer는 원래 인증과 같은 Zeroizing 값으로 잠시 보관한다. native host가
Authorization 헤더를 붙이고 worker·material·Receipt·모델에 값을 전달하지 않는다. 프로세스 도구에
전달되는 기존 safe environment 변수는 인증 원본으로 선택할 수 없다. 필터 목록을 중복 정의하지 않고
기존 process 환경 선택 규칙에서 판정한다.

manifest에 기본 생략 가능한 httpBearerOverrides 참조 map을 추가한다. execution profile의 HTTP
adapter binding도 참조 map에 결합한다. 토큰 값이나 값의 hash는 넣지 않는다. 미완료 Run의 resume은
동일 map을 다시 지정해야 한다. 누락·변경은 http_bearer_overrides drift로 거절한다. 같은 env 이름의
토큰 회전은 허용한다. 완료 Run은 기존처럼 이 검사보다 먼저 offline replay한다.

override가 없으면 새 필드를 직렬화하지 않고 기존 adapter binding/profile을 유지한다. Capability
2.0.0과 HTTP profile v3를 다시 올리지 않는다. 새 동작은 명시된 참조 map으로만 profile에 들어간다.

## 호스트 책임

서버는 사용자별 토큰을 각 자식 프로세스의 환경에 넣고 Run을 실행한다. 같은 프로세스의 전역 환경을
요청마다 바꾸는 방식은 다중 사용자 격리가 아니다. Run ID 접근 제어·사용자 인증·사용자별 상태 저장소·
토큰 발급·대리 권한 검증은 호출 서버의 책임이다. 이 기능만으로 서버 통합이 완료됐다고 주장하지 않는다.

## 검증

shipment(저장된 bearer 참조 있음)과 sensor(원래 인증 없음)를 별도 loopback API/결정적 모델로
호출해 Run별 Authorization·연결 원문 불변을 확인한다. 한 Run에 연결 두 개와 서로 다른 토큰도
검증한다. 실제 토큰 값이 모든 상태 파일·모델 요청에 없고 credential echo는 버려지는지 확인한다.

승인 pause 이후 참조 누락·변경·환경변수 부재에서 HTTP0회, 동일 참조의 토큰 교체 뒤 성공, 완료
Run의 환경변수 없는 replay에서 추가 API/모델0회를 검증한다. manifest 무결성·profile 참조 결합·
기본 empty map 호환성을 native unit 검사로 검증한다. 실제 외부 서비스·LLM 품질·서버 tenant 격리는
측정하지 않는다. 기존 v1 HTTP binary의 미완료 거절·완료 replay 회귀도 유지한다.

승인 있는 쓰기는 사용자 요청대로 대회 후 별도 설계다.

Rust784개(4 ignored), pinned backend+컴파일 CLI의 Python153개(7 skipped), Clippy·fmt·protocol·문서·workflow 검사를 통과했다.
