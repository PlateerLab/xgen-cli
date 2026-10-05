# 내부 도구 검색 계약과 첫 구현

작성: 2026-10-06 (KST). 범위: 도구 발견·그래프 저장·검색·계약 조회. 업무 도구 실행과 agent loop 자동 검색은 후속 작업이다.

## 사용

```bash
xgen tools import --name inventory --source ./openapi.json
xgen tools search --name inventory "read asset detail"
xgen tools describe --name inventory getAssetDetail
xgen tools list
```

`--source`는 여러 JSON 명세 파일 또는 공개 HTTPS OpenAPI/Swagger UI 주소를 받는다. 로컬 JSON은 문서당 5 MB, 입력은 최대 16개다. URL에 인증값을 넣지 않도록 userinfo·query·fragment는 거절한다. 인증이 필요한 명세 수집은 아직 제공하지 않는다. 원격 명세 수집은 graph-tool-call의 기본 네트워크 검증을 사용하며 private-host 허용 옵션을 제공하지 않는다.

CLI가 바이너리에 포함한 Python worker를 stdin/stdout JSON으로 자동 호출한다. uv가 Python 3.12와 `graph-tool-call==0.46.0`을 관리한다. 별도 서버·포트·pip 설정은 없다. **현재 개발 버전은 PATH의 uv가 필요하고 최초 준비에는 네트워크가 필요하다.** uv 자체를 포함하는 OS별 배포는 후속 TODO다. uv 설정·프로젝트·`.env` 로딩과 Python 경로 주입을 차단하고 모델 API 키 등은 worker 환경에 넘기지 않는다.

요청·출력·시간 제한: 요청 64 KiB, worker 응답 16 MiB, 전체 worker 실행 120초. 시간·출력 한도를 넘으면 process group 또는 Windows Job Object를 종료한다. 상세 upstream 오류·stderr는 표시하지 않고 오류 코드만 반환한다. 공개 HTTPS 주소라 해도 업무 API를 호출하는 기능은 없다. 처음 준비하는 패키지 다운로드는 이 제한 시간에 포함된다.

## 저장과 공통 도구 계약

Run과 같은 OS별·legacy 호환 state root 아래 `tool-collections`에 저장한다. `XGEN_STATE_HOME`도 기존 규칙을 따른다. Unix collection 디렉터리는 사용자 소유·0700이어야 한다. 완성된 임시 파일을 hard link로 공개하고 같은 이름은 덮어쓰지 않는다. 현재는 갱신·삭제 명령을 제공하지 않으며 새 이름으로 새 snapshot을 저장한다.

저장 envelope는 `format_version=1`, collection 이름, backend·version, artifact digest, upstream artifact로 구성한다. digest는 worker의 정렬된 ASCII JSON 직렬화에 대한 SHA-256이며 JCS나 서명된 Receipt는 아니다. 로드 시 검증해 손상된 artifact의 사용을 거절한다. 원본 snapshot manifest와 로컬 명세의 내용 digest·파일명도 보존한다.

저장 파일은 `.json.gz`다. 명세 입력의 5 MB 제한과 별개로, 그래프는 압축 전 256 MiB·압축 후 64 MiB 한도를 둔다. 압축 해제도 한도 이상 읽지 않는다. 실제 대규모 명세에서 반복되는 계약 정보 때문에 64 MiB 저장 제한을 넘는 사례가 있어 이 경계를 분리했다. hard link를 지원하지 않는 저장소는 `collection_publish_unavailable`로 실패한다.

발견한 도구의 참조는 **collection + artifact digest + tool name**이다. upstream 이름 자체를 곧바로 xgen Capability ID로 인정하지 않는다.

`describe`는 정규화된 parameters와 보존된 `metadata.api_contract`를 반환한다. 이 계약에는 원본 HTTP 파라미터·입출력·security 등의 정보가 있으며 실행용 schema로의 무손실 매핑은 별도 검증이 필요하다. 현재 결과는 `contract_kind=discovery_candidate`, `effect_class=unclassified`, `execution_enabled=false`다.

실행을 연결할 때는 기존 `CapabilityDefinition`에 정확한 입력·출력·효과·검증 계약을 등록하고, `CapabilityInstance`에 adapter·target·auth reference를 연결한다. 스키마·효과·검증 의미가 같을 때만 같은 Capability ID를 공유한다. 외부 annotation으로 읽기 전용이나 승인을 추정하지 않는다. 발견 결과의 digest를 Run의 계약과 묶고 실행 직전에 재검증한다. 이 admission·실행 매핑은 아직 구현하지 않았다.

`search`는 상위 후보 1–20개와 최대 한 hop의 가능한 producer 도구를 반환한다. producer 표시는 최대 40개이고 나머지 개수를 명시한다. producer 관계는 실행으로 검증된 순서가 아니며 후보 포함이 실제 사용 가능성·권한을 증명하지 않는다. 후보 전체 스키마는 `describe`로 별도 조회한다.

## 화면 관찰 계약: 설계

아직 브라우저 실행 코드는 연결하지 않았다. 다음 필드는 공통 adapter 계약 후보다.

| 관찰 | 필드 |
| --- | --- |
| 관찰 정체성 | session, page, snapshot generation, observed time |
| 화면 요소 | 임시 ref, role, accessible name, 위치·표시 상태 |
| 작업 정의 | 입력·출력, 화면 선행 조건, 동작 binding, 성공 관찰 조건 |
| 관계 근거 | source snapshot, 관찰/추론/실행 검증 상태 |

영구 tool ref와 현재 화면 element ref를 분리한다. generation이 맞지 않으면 재관찰하며, 클릭 동작 자체를 read-only로 분류하지 않는다. 일반 그래프가 오래된 `eN` ref를 재사용하게 만들지 않는다.

## 역할별 모델 계약: 설계

모델 역할과 실제 endpoint/checkpoint를 분리하고 같은 모델의 여러 역할 공유를 허용한다. 초기 구현은 모델 호출을 추가하지 않는다.

| 역할 | 입력 | 결과와 검증 |
| --- | --- | --- |
| 추론·응답 | 목표, 검증된 컨텍스트, 후보 도구 계약 | 기존 PlanProposal/response/completion 계약 유지 |
| 판단 | 현재 관찰, 질문, 허용 후보 ID | SEV의 typed choice; 후보 밖 선택 거절; confidence는 권한이 아님 |
| STT | audio session과 순서가 있는 청크 | 누적 partial / final / request boundary를 구분; 중복 실행 방지 |
| 화면 해석·TTS | 필요한 관찰 또는 확정 응답 | 선택 기능으로 별도 연결; 텍스트 실행의 필수 의존성으로 만들지 않음 |

role binding에 backend/model/version, timeout, 호출·비용 예산을 연결할 예정이다. 실패·보류·사용자 질문을 분리하고 자동 효과 재실행으로 연결하지 않는다. API 선택과 UI 선택에 서로 다른 SEV 체크포인트를 쓸 수 있다.

## 검증 계획과 채택 조건

첫 검증은 LLM이 없는 결정적 통합 검사다. 성능 향상을 주장하는 비교 실험이나 SEV 모델 비교는 아니다.

- 설계 사례: OpenAPI 3.1 자산 상세, Swagger 2.0 일정 상세. 실제 pinned 라이브러리로 import/search/describe, parameter·api_contract 보존, producer 후보와 artifact 정체성 확인.
- 보류한 CLI 검증 요청: 자산 목록, 일정 목록. 상세 요청과 다른 기대 도구를 CLI에서 선택한다. 기대값·top_k=1을 첫 CLI 실행 전에 테스트 코드에 고정했다.
- 실패 검사: 중복 import의 기존 bytes 보존, artifact 변경 후 거절, 이름 경로 탈출·없는 도구·없는 collection·빈 요청·후보 수 한도·중복 JSON key 거절.
- 조회·검색에서 업무 API 실행과 모델 호출은 0회다. fixture 테스트를 실제 두 시스템 연결 검증이라고 보고하지 않는다.
- agent loop 적용·기본값 변경 전에 기존 전체 목록 경로와 새 검색 경로를 비교한다. 새 시스템/요청으로 사례를 분리하고 후보 포함률·작업 완료율·잘못된 실행·시간·토큰·금액 및 사전 채택 기준을 고정한다. 이번 단계에서 기본 모델·기존 Run 동작을 변경하지 않는다.

검증 명령:

```bash
cargo build -p xgen-cli
XGEN_TOOL_TEST_BINARY="$PWD/target/debug/xgen" \
  uv run --no-project --no-config --no-env-file --isolated \
  --python 3.12 --with graph-tool-call==0.46.0 -- \
  python -m unittest discover -s scripts/tests -p test_tool_graph_worker.py -v
cargo test -p xgen-cli
cargo clippy -p xgen-cli --all-targets -- -D warnings
```

Python 검사는 별도 pinned 환경이 없으면 library 검사를 skip하며, CLI 검사는 binary 환경변수 지정 시 실행한다. 실제 통합 검증에서는 두 옵션을 모두 설정해야 한다.

## 확인한 결과

- 실제 pinned worker와 compiled CLI 검사 8개 통과. OpenAPI 3.1·Swagger 2.0의 상세·목록 검색과 schema 조회, producer 후보, 변조·덮어쓰기·압축 해제 한도를 확인했다.
- 신규 Rust lifecycle 검사 2개와 transport 검사 5개 통과. stdin을 읽지 않는 프로세스의 timeout, leader 종료 뒤 남은 자식 정리, 키·Python 주입 환경 차단, stderr 비노출, 출력 한도, URL credential 거절, Ctrl+C를 확인했다. Unix/Linux 범위의 검증이며 Windows 실측은 하지 않았다.
- xgen-cli 패키지 회귀 검사와 Clippy 통과. 새 기능이 모델 호출·업무 API 실행을 추가하지 않음을 구현 경계로 유지했다.
- 기존 실제 OpenAPI snapshot을 새 CLI로 import/search했다. 도구 1,108개, gzip 저장 6,860,637 bytes, import 6.79초·search 3.44초. 최초 저장 제한 64 MiB에서 실패한 뒤 압축 전/후 한도를 분리했다.
- 공개 Swagger UI 주소를 새 CLI에 직접 넣는 수집도 통과했다. 명세 15개·도구 1,108개, import 10.68초. 이 검증은 명세 GET과 그래프 생성이며 업무 API 실행은 아니다. 시스템별 주소·원본·상세 결과는 로컬 비공개 기록에 둔다.
- 검색마다 그래프를 다시 로드하므로 이 측정은 검색 계산만의 시간이 아니다. 내부 캐시와 장기 유지 worker는 후속 성능 과제다. 기존 snapshot의 검색 첫 후보는 요청 의미를 충분히 구분하지 못했으며 품질 개선을 주장하지 않는다.
