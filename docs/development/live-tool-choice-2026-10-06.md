# 실제 모델의 검색어·도구 선택 분리 평가

## 얻은 결과와 판정 범위

실제 DeepSeek로 평가를 완료했다. 모호한 정답 기준의 1개 intent를 결함 판정에서 제외한 **사후 민감도 분석**에서는, 원래 요청으로 검색했을 때 28회 중 16회, 모델이 생성한 검색어로 검색했을 때 26회에서 정답을 골랐다. 정답 후보가 주어진 조건에서는 각각 **16/16·26/26**이다. 현재 확실한 병목은 선택 이후의 xgen 전송 손실이 아니라 검색 후보 누락이다.

이것은 새 검색어 생성 baseline의 관찰 결과다. 엔진·검색기·기본 모델·prompt를 개선한 전후 성능 비교가 아니다. 정답 기준을 사후에 바꿔 원점수를 올리지 않았다. 첫 등록의 원점수와 한계는 아래에 그대로 남긴다. 실제 xgen 전체 agent의 업무 완료·응답 정확성·현재 HTTP adapter 실행 성공률도 이번 측정 범위 밖이다.

## 조건·원점수

[사전 고정 계획](live-tool-choice-plan.md)을 따른다. Gitea/WireMock의 이전 intent와 다른 8개 조회 intent와 없는 작업 2개를 영어·한국어로 작성했다. 20개 요청 × 2회 = 40개 trial, trial당 query 생성·원래 후보 선택·생성 후보 선택의 실제 모델 호출 3회, 총 **120회**다. 실제 모델 호출 전에 source·binary·worker·prompt·판정·view hash·순서·예산을 등록했다. a/b는 유료 호출 없는 미완료 준비 실행이고, 최종 c만 평가 근거로 사용한다.

원래 요청/생성 검색어 후보의 선택에는 **동일한 모델용 describe 2.0.0 전체 view**를 제공했다. 추가 요약·잘림·label 주입은 없다. semantic discovery를 평가하므로 승인·execution_enabled·HTTP subset 지원 여부를 의미 매칭 기준으로 사용하지 않도록 설명했다. 업무 API는 호출하지 않았다.

| 사전등록 원점수 | 원래 요청 후보 | 생성 검색어 후보 |
| --- | ---: | ---: |
| 조회 trial | 32 | 32 |
| 주 정답/허용 대체 포함 top-5 | 18/32 | 26/32 |
| 주 정답/허용 대체 선택 | 16/32 | 26/32 |
| 허용 정답 후보가 있을 때 선택 | 16/18 | 26/26 |
| 없는 작업에서 null 거절 | 8/8 | 8/8 |
| 형식 오류·후보 밖 선택 | 0/40 | 0/40 |

허용 대체 도구까지 포함한 지표와 주 정답만 인정한 지표는 이번 결과에서 동일했다. 데이터 공급 대체는 exact filtering을 전제로 등록했고, 실제 실행·filtering의 성공은 확인하지 않았다.

| 시스템·언어 | 원래 후보 정답 포함/조회 8회 | 생성 후보 정답 포함/조회 8회 | 원래 정답 선택 | 생성 정답 선택 |
| --- | ---: | ---: | ---: | ---: |
| Gitea 영어 | 6/8 | 6/8 | 4/8 | 6/8 |
| Gitea 한국어 | 4/8 | 6/8 | 4/8 | 6/8 |
| WireMock 영어 | 4/8 | 6/8 | 4/8 | 6/8 |
| WireMock 한국어 | 4/8 | 8/8 | 4/8 | 8/8 |

등록 기준에서는 생성 검색어로 정답 포함을 10회 회복했고 2회 잃었다. 반복 선택은 두 조건 모두 20쌍/20쌍 동일했지만, 생성 query 문자열은 15/20쌍·생성 후보 ID/순서는 18/20쌍만 동일했다. temperature=0을 query·검색 결과의 완전한 재현성으로 해석하지 않는다. 40개 trial은 40개 독립 업무가 아니고, 같은 10개 intent의 언어·반복 조합이다.

## 정답 기준의 결함과 사후 민감도 분석

`gitea-branch-tip`은 main 브랜치의 최신 커밋 정보를 요청하면서 `repoGetBranch`와 `repoListBranches`만 허용했다. 원본의 `repoGetSingleCommit` 파라미터 sha 설명은 **a git ref or commit sha**다. 명세상 main 같은 ref로 Commit 데이터를 조회할 수 있으며, branch는 PayloadCommit 정보를 제공한다. 상세 정도의 요구도 충분히 명확하지 않았다. 실제 한국어 생성 후보 선택이 repoGetSingleCommit으로 나온 두 trial을 틀렸다고 평가한 기준은 불완전하다.

원본 근거: [고정 Gitea 명세](https://github.com/go-gitea/gitea/blob/08c6ea6728ee35a16f9a39588e725044b7f389cd/templates/swagger/v1_json.tmpl), `/repos/{owner}/{repo}/git/commits/{sha}`와 definitions Commit/Branch/PayloadCommit. 원본 response는 최상위 responses ref도 포함하므로 정답 작성 시 parameter와 response ref를 함께 해제해 확인해야 한다.

이후 label을 수정해 점수를 다시 계산하거나 추가 유료 호출을 하지 않았다. 이 intent의 영어/한국어 × 2회, **4개 trial 전체**는 모델/view/엔진 결함 판정에서 제외했다. 원점수는 유지하고 사후 민감도 분석으로만 다음을 제시한다.

| 사후 분석: 나머지 7개 조회 intent·28회 | 원래 요청 후보 | 생성 검색어 후보 |
| --- | ---: | ---: |
| 정답 포함·선택 | 16/28 | 26/28 |
| 정답 후보가 있을 때 선택 | 16/16 | 26/26 |

이 제외는 결과를 본 뒤 수행했으므로 새 사전등록 개선 증거가 아니다. 다음 평가에는 데이터·입력 계약이 명확한 새 요청과 원본 전체 ref에 근거한 정답/대체 도구 audit가 필요하다. 이 기준 결함을 xgen 코드나 특정 도구의 순위 보정으로 해결하지 않는다.

## 실패 소유자와 다음 작업

- **graph-tool-call 검색/색인:** 원래 요청에서 사용자 프로필(Gitea), 미사용 스텁과 가까운 매칭(WireMock) 누락이 직접 검색에서도 재현됐다. query 생성으로 여러 누락이 회복됐지만 실제 모델 query에서도 WireMock 영어 near-misses 2회는 후보 누락이 남았다. 고정 source·query·backend의 재생 근거이며 단독 검색기 결함이나 모델 query만의 결함으로는 아직 확정하지 않는다.
- **모델 query·xgen 요청 구성:** request를 적절한 검색 표현으로 만드는 가치가 두 시스템에서 관찰됐다. 이번 query 생성기는 독립 평가 코드다. production planner가 동일하게 동작한다는 증거가 아니므로 다음에 실제 xgen search proposal을 별도 재생·비교해야 한다.
- **선택 모델/view:** 명확한 7개 조회 intent에서는 정답 후보가 있을 때 선택 오류가 없었다. 현재 결과만으로 모델 교체·SEV 학습·큰 schema 상세 조회를 우선 채택할 근거는 없다. 7개 intent의 작은 표본이고 더 복잡한 입력 생성·출력 판단은 미평가다.
- **xgen 전달:** 새 20개 요청의 직접→CLI→agent search/첫 후보 describe 차이 0건. 생성 query 40개의 직접→CLI 후보도 동일했다. loopback driver Receipt는 search 20개·describe 19개, **39개 모두 succeeded·검증 passed**다. 없는 작업 한 요청에서 후보가 비어 describe를 수행하지 않았다.
- **평가 운영:** branch-tip의 불완전한 대체 정답 기준을 먼저 개선해야 한다. 이미 본 20개 요청은 이후 설계 자료로 취급한다.

다음 개발은 **실제 xgen planner의 검색어 구성 재생 + graph-tool-call의 범용 검색/색인 ablation**이다. 두 책임을 구분하고, 정답 계약을 audit한 새 held-out 요청으로 확인한다. 도메인 전용 synonym·순위 보정은 엔진에 넣지 않는다. 현재 baseline만으로 production query rewrite·임베딩·reranking 기본값을 바꾸지 않는다.

## 호출·비용·재현 근거

모델 alias `deepseek-flash`, 응답 fingerprint `aeb56401ca74e127821c4f9126dcb669`는 120회 동안 동일했다. 입력 token **392,668**, cache hit **188,416**, 출력 **1,031**이고 미해결 예약은 0이다. [DeepSeek 공식 가격표](https://api-docs.deepseek.com/quick_start/pricing/)의 peak 단가를 상한으로 적용한 계산은 **$0.063643296**, 사전 예산 $0.20 이하다. off-peak 할인·실제 청구 내역은 적용/확인하지 않았으며 delegate 리뷰/분석 호출의 비용은 포함하지 않았다.

API call median은 query 0.592초·원래 후보 선택 0.612초·생성 후보 선택 0.624초다. 순서는 항상 fixed 선택 후 generated 선택이며 cache 조건이 달라서 이 시간을 두 선택 방식의 성능 우열로 해석하지 않는다. 전체 trial 시간이나 production 지연도 아니다.

[실행기](../../scripts/evaluate-tool-choice.py), [case 정의](../../scripts/fixtures/tool-choice-evaluation/cases.json), [등록](../../scripts/fixtures/tool-choice-evaluation/registration-20261006.json), [실제 호출 순서](../../scripts/fixtures/tool-choice-evaluation/live-registration-20261006.json), [집계](../../scripts/fixtures/tool-choice-evaluation/summary-20261006.json), [분석·사후 구분](../../scripts/fixtures/tool-choice-evaluation/analysis-20261006.json), [개별 결과와 payload hash](../../scripts/fixtures/tool-choice-evaluation/live-rows-20261006.json)를 저장했다. 사전등록 SHA-256은 `4d2bb81a17c390392283a172487c84b3300d39d25398a2d319bbb006dba981a1`이다.

전체 source·graph·Run/Receipt·실제 payload·ledger는 로컬 실험 디렉터리에 보관했다. 키·인증 header·환경 파일은 결과에 포함하지 않았다. 새 평가 검사 6개·기존 도구 평가 검사 5개·기존 모델 평가 검사 11개 통과, built example 1개 skipped, 공개 문서 검사 통과. 엔진 변경이 없어 Rust/runtime 재빌드는 하지 않았다.

```bash
uv run --offline --no-project --no-config --no-env-file --isolated \
  --python 3.12 --with graph-tool-call==0.46.0 -- \
  python scripts/evaluate-tool-choice.py --binary target/release/xgen \
  --output /tmp/xgen-tool-choice-new --phase prepare --freeze
# 로컬에서 XGEN_TOOL_EVAL_API_KEY를 지정한 뒤 같은 output에 --phase live로 실행한다.
# live-registration이 생성된 output은 자동 재시도·재개하지 않는다.
```
