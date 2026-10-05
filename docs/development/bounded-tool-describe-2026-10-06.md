# 큰 명세 전달 수정 결과

## 얻은 결과

일반적인 xgen 전달 경로 변경이다. 기존 Gitea/WireMock 32개 설계 회귀 요청에서 describe 전달 실패는 **14/32 → 0/32**다. 모델에는 최대 **6,641 bytes**의 선택 view를 제공하고, 호스트는 전체 원본 HTTP 계약으로 입력·응답을 검증한다. 검색 알고리즘·기본 모델·업스트림 코드는 변경하지 않았다.

| 검사 | 이전 | 이번 |
| --- | --- | --- |
| agent describe 실패 | 전체 후보 조회 14/32 | 제한된 view 조회 0/32 |
| 직접 → CLI 검색/전체 후보 차이 | 0/32 | 0/32 |
| CLI → agent search 차이 | 0/32 | 0/32 |
| agent describe의 기대 표현과 차이 | 전체 후보 기준 14/32 | CLI model-view 기준 0/32 |
| 목표 endpoint top-5 | 18/32 | 18/32 |

이번 view는 전체 schema와 동등한 표현이 아니다. 생략 여부·전체 후보 digest·HTTP 계약 digest를 명시한다. 전체 CLI 후보는 여전히 직접 graph-tool-call 결과와 동일하고, 모든 view의 전체 후보/지원 HTTP 계약 digest도 원본과 일치했다. 위 32개는 이미 관찰한 요청이므로 새 품질 검증 사례라고 부르지 않는다.

## 범위와 근거

[계약·사용법·한도·버전 이행](bounded-tool-describe.md), [검사 전 고정 계획과 수정 이력](bounded-tool-describe-plan.md)을 따른다. 기존 동결 결과는 수정하지 않고 새 binary/worker/실행기/계획/검사 코드로 조건 b를 재등록했다. 중간 조건 a는 미지원 후보의 normalized type/enum 표시를 수정하면서 결과 완성 전에 중단했고 채택 근거로 사용하지 않았다.

- Gitea 448개·WireMock 40개 후보, 같은 32개 고정 요청·top-k 5·각 3회 검색. 반복 안정성 32/32, 전체 CLI describe 최대 1,196,417 bytes, agent view 최대 6,641 bytes.
- 결정적 loopback 모델 요청 96회, search/describe Receipt 64개 모두 succeeded·검증 passed. 실제 업무 모델 호출·업무 API 호출은 0회다. 별도의 delegate diff 리뷰 호출은 이 평가의 호출/비용 통계에 포함하지 않았다.
- 언어별 목표 endpoint top-5는 Gitea 영어 4/8·한국어 2/8, WireMock 영어 8/8·한국어 4/8로 이전과 같다. 원본에 없는 scope 설명 주입과 description의 색인 표현 문제도 이번에 수정하지 않았다.
- 직접 search median 0.170초, 직접 load+search 0.396초, CLI 전체 search 0.903초. 측정 범위가 달라서 순수 전달 overhead나 성능 개선으로 해석하지 않는다.

사전등록 SHA-256: `dbbb06e9f2d92519865eff2894731ce71d913bf8c91a1543e1a76337e04b668d`.

공개 근거: [등록](../../scripts/fixtures/tool-search-evaluation/bounded-registration-20261006.json), [집계·개별 요청 결과](../../scripts/fixtures/tool-search-evaluation/bounded-results-20261006.json). 전체 원본·그래프·모델 컨텍스트·Run/Receipt는 로컬 실험 디렉터리에 보관하고 저장소에는 집계만 넣었다.

## 별도 검증 사례와 회귀 검사

처음 도서/장비 사례에서 승인 코드의 describe 버전 1.0.0 고정 때문에 실행이 차단됐다. 버전별 승인 조건 수정 후 이 입력은 설계 회귀로 전환했다. 그 뒤 결과를 보기 전에 고정한 실험 배치(OpenAPI 3.1·array 응답·components ref)와 녹음 기록(Swagger 2·enum 응답·definitions ref) 두 입력에서도 다음을 확인했다.

- 큰 원본 schema로 정상 GET 1회·검증된 응답·완료 재생, 재생 추가 GET 0회.
- view에 생략된 입력 enum 위반과 계약 digest 위조는 GET 0회로 차단.
- view에 생략된 응답 schema의 minItems/enum 위반은 실패하고 검증되지 않은 body를 최종 응답에 노출하지 않음.

70개 입력의 페이지 조회는 순서·누락·중복 없이 통과했다. 음수/bool/초과 offset, 버전·입력 digest 변경을 거절하고 control/Unicode 텍스트에서도 worker view 48 KiB 한도를 확인했다. 미지원 HTTP 후보의 정규화 type/enum과 원본 schema의 출처도 구분했다.

최종 CLI Rust **179개**·protocol **13개**, Python 도구 **34개**, 평가 실행기 **5개**, 기존 모델 평가 실행기 **11개** 통과. Rust live 3개·built example 1개는 미실행이다. Clippy·fmt·공개 문서 검사 통과. release에서도 새 대형 계약 검사 5개를 재확인하고 로컬 실행 바이너리를 갱신했다. 이전 버전의 완료 Run 재생과 진행 중 Run의 execution_profile 불일치 거절도 실제 저장 Run으로 확인했다.

## 남은 과제와 소유자

- **xgen·평가:** 전달 gate는 이번 조건에서 합격했다. 새 검증 요청·허용 대체 도구·비용/예산 기준을 고정한 뒤 실제 LLM의 검색어 생성과 후보 선택을 분리 평가한다. 32개 설계 요청만으로 일반 품질 개선을 주장하지 않는다.
- **xgen:** 큰 enum 값·중첩 schema의 세부 조회가 필요하면 전체 계약 digest에 연결된 제한된 조회를 추가한다. 현재 입력 페이지는 파라미터 목록 페이지이고 schema 상세 페이지는 아니다. 의미 정보 생략이 선택에 주는 영향은 아직 미평가다.
- **graph-tool-call:** 직접 검색의 누락·언어 차이·scope 설명 주입은 해당 검색/정규화 경로에서 ablation 후 개선한다. xgen에 도메인별 순위 보정 규칙을 넣지 않는다.
- **모델·SEV:** 올바른 후보·필요한 입력이 전달된 뒤의 선택·입력 생성 품질을 평가한다. 이번 결정적 fixture 검사를 실제 LLM 품질 향상으로 해석하지 않는다.
