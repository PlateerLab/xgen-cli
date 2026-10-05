# 큰 도구 명세의 모델 전달: 검증 계획

일반 변경이다. 검색·색인·순위는 그대로 두고 모델용 `xgen.tools/describe@2.0.0`를 제한된 view로 분리한다. 실행 호스트는 저장한 전체 원본으로 파라미터·응답을 검증한다. view는 완전한 계약이라고 표시하지 않는다.

검사 전에 고정한 조건:

- 기존 Gitea/WireMock 32개는 이미 관찰한 **설계 회귀 사례**다. 새 binary·worker·평가 실행기·이 문서 hash로 별도 등록한다. 이전 등록·결과는 바꾸지 않는다.
- 직접 graph-tool-call → CLI 원본 describe 비교는 유지한다. agent describe는 `tools describe --model-view`와 전체 view가 같아야 한다. 원본과 다른 view를 정보 손실로 판정하거나 원본 전달 성공이라고 부르지 않는다.
- search 후보·순서·점수는 동일, agent view는 64 KiB 미만, 전체 normalized tool digest·지원되는 HTTP 원본 계약 digest는 CLI 원본과 같아야 한다. 실패 Receipt가 사라졌는지 확인한다.
- 새 검증 입력은 OpenAPI 3.1의 도서 조회와 Swagger 2.0의 장비 지표 조회다. 큰 description·큰 enum·큰 response schema를 포함한다. 개발에 사용한 기존 fixture와 별개로 첫 실행 전에 정의하고 고정한다. 실제 유료 모델·업무 API 대신 loopback fixture 모델·HTTP 서버를 사용한다.
- 새 입력에서 정상 조회·Receipt·완료 재생, view에 생략된 enum 제한 위반의 **전송 전 차단**, 생략된 응답 schema 위반의 실패를 확인한다. response schema가 커도 호스트 검증을 생략하지 않는다.
- 70개 입력의 페이지를 끝까지 조회하여 누락·중복 없는 순서와 digest를 확인한다. offset·버전·페이지 위조는 거절해야 한다. 통제된 대형/비ASCII 텍스트도 bytes 기준으로 제한한다.

이 검증은 전달·실행 계약의 정확성을 확인한다. 실제 LLM 선택·검색 품질 개선을 주장하지 않는다. 검증 사례를 본 뒤 구현을 고치면 그 입력은 이후 회귀 사례로 취급하고 새 검증 입력을 추가한다.

첫 실행에서 도서/장비 HTTP 검증은 새 describe 버전의 승인 조건 누락으로 차단됐다. 승인 버전 조건 수정 후 두 입력은 설계 회귀로 전환했다. 수정 후 결과를 보기 전에 OpenAPI 실험 배치(array 응답·components ref)와 Swagger 녹음 기록(enum 응답·definitions ref)을 새 검증 사례로 고정했다. 입력 enum 차단·원본 응답 제약 실패·digest 위조 차단·정상 재생 조건은 동일하다. 초기 테스트 설정의 server helper 누락도 수정했다.

중간 검토에서 HTTP 미지원 후보의 normalized type/enum을 view에 포함해야 함을 확인해 수정했다. 첫 사전등록 실험 a는 결과 완성 전에 중단했고 채택 근거로 사용하지 않는다. 새 실험 b로 재등록한다. 원본 schema와 정규화 type/enum의 `schema_origin`을 구분한다. 이 수정은 실행 지원 후보의 HTTP 계약·승인·검증 조건을 변경하지 않는다.
