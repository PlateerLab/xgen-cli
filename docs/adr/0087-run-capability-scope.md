# ADR-0087: Run별 capability 범위 고정

- 날짜: 2026-11-19
- 상태: 채택, 구현·로컬 검증 완료
- 범위: 일반 호스트 설정. 도메인별 도구 선택 규칙이나 모델의 자동 검색은 포함하지 않는다.

## 문제와 결정

일부 도구만 필요한 실행에도 전체 capability schema가 planning context에 들어갔다.
`xgen run --capability ID`를 반복 지정하면 원래 등록 가능한 정의와 instance의 부분집합만
CapabilityRegistry에 등록한다. 같은 registry를 모델 카탈로그와 proposal admission에 사용한다.
숨긴 도구의 제안은 실행 전에 거절한다. API 원문·tool output·Receipt 내용은 축약하지 않는다.

목록은 최대128개·ID256 bytes, 정렬·중복 검사·가용성 검증을 거친다. 잘못되거나 등록되지 않은 ID는
Run 생성·모델 호출 전에 거절한다. 기본 empty 입력은 None이며 기존 전체 카탈로그를 유지한다.
선택은 권한 부여가 아니다. 선택된 write도 별도 write 승인이 필요하고, 범위 밖 write는 승인 여부와
무관하게 admission에서 거절한다.

정렬한 scope를 manifest의 선택 필드 capabilityScope와 authority·execution profile digest에
결합한다. resume은 새 범위 옵션을 받지 않고 저장된 범위를 복원한다. 기본 None은 직렬화에서
생략하고 기존 profile digest를 유지한다. 완료된 Run의 offline replay도 유지한다.

## 안내와 필수 조건의 일치

등록된 capability의 ID·resource selector에서 planning 안내의 적용 대상을 유도한다.
파일 도구가 없으면 workspace 안내를, process 도구가 없으면 실행 파일 안내를 보내지 않는다.
명시적 파일 subset에는 경로 범위만 알려 주고 개별 도구 계약은 선택된 definition에 맡긴다.
terminal·web만 제공한 Run에는 executable ID 목록을 알리되 process.execute 사용을 지시하지 않는다.
search/describe는 각각 실제 제공 여부에 맞춰 안내하며, search가 없으면 검색어 언어 힌트 조회도 생략한다.
기본 전체 카탈로그의 안내 문장은 기존과 동일하다. 명시적 범위와 기존 constrained composition의
조합에는 범위 안내를 추가해, 다른 안내가 모두 제외되어도 provider의 nonempty constraint 계약을 지킨다.
정확한 allow-file 모드와 제외된 실행 파일 catalog를 함께 쓰는 경우에도 실행·재개가 가능하다.

HTTP read만 선택할 수 있다. 호스트가 정확한 contractDigest·입력을 제공하면 원본 계약으로 검증한다.
필요한 describe가 없고 계약도 없으면 선행조건 부족을 보고해야 한다. 계약을 추측해 호출할 권한은 없다.

completion contract의 필수 process 검사가 있는데 process.execute를 제외한 설정은 모순이다.
이 조합은 모델 호출 전에 거절한다. 일반 선택 범위에 임의의 선행 도구를 자동 추가하지 않는다.

## 검증과 한계

설계 검사에서 read-text/stat의 승인 대기·재개·동일 catalog·완료 replay, 선택된 write의 별도 승인,
범위 밖 write 차단, 모르는/미등록/중복 ID·manifest 변조·필수 검사 모순을 검증한다.
선택하지 않은 process·write 안내와 필수 검사 충돌은 수정 전 실행에서도 실패를 재현했다.

별도 shipment/sensor fixture의 실제 loopback API·결정적 모델에서 HTTP read-only/describe-only
범위와 안내 일치, 기본 대비 요청 크기 감소, 동일 API 입력·응답 본문 유지를 확인한다.
실제 LLM의 판단 품질·성공률·금액 절감은 측정하지 않는다. 큰 본문·누적 기록은 별도로 입력 한도를 넘을 수 있다.

독립 diff 검토 도구는 인증 오류로 실행하지 못했다. 코드 직접 검토와 native·CLI 회귀 검증으로 확인한다.

최종 로컬 검증: Rust798개 통과(4 ignored), system Python3.14+pinned graph-tool-call0.47.0+컴파일 CLI의
Python162개 검사 통과(9 skipped), Clippy·fmt·protocol·문서·workflow 검사 통과. 이전 rc.6 바이너리로
시작한 미완료 HTTP Run도 두 fixture에서 새 바이너리로 동일 catalog·API 계약을 유지해 재개했다.
Python3.14 검사 중 SQLite ResourceWarning이 출력됐지만 검사 실패는 없었다.
