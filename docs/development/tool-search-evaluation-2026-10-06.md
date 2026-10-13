# 도구 검색·전달 책임 분리 평가 결과

2026-10-06. 이번 검증에서는 **검색 후보 전달은 같았지만 검색 누락과 큰 명세의 agent 전달 실패가 따로 재현됐다.** 런타임·검색 알고리즘은 수정하지 않았다.

## 조건과 재현

[사전 계획](tool-search-evaluation-plan.md), [동결 manifest](../../scripts/fixtures/tool-search-evaluation/registration-20261006.json), [입력 요청](../../scripts/fixtures/tool-search-evaluation/cases.json), [결과·실패 Receipt 분석](../../scripts/fixtures/tool-search-evaluation/results-20261006.json)을 함께 관리한다. 실행기는 [evaluate-tool-search.py](../../scripts/evaluate-tool-search.py)다.

- [Gitea v1.24.6](https://github.com/go-gitea/gitea/blob/08c6ea6728ee35a16f9a39588e725044b7f389cd/templates/swagger/v1_json.tmpl): 원본 452 operations, 기본 skip_deprecated 뒤 448개 도구.
- [WireMock 3.13.1](https://github.com/wiremock/wiremock/blob/1a9a61507fbca5c7d6da72de4b22d54ceef48760/src/main/resources/swagger/wiremock-admin-api.json): 원본 41 operations, 같은 조건에서 40개 도구.
- 원본은 고정 commit·raw SHA256로 검증하고 전체 명세를 그대로 가져왔다. graph-tool-call 0.46.0의 기본 keyword/graph 검색, embedding 없음, top-k=5. 직접 호출과 CLI 비교는 같은 artifact에서 요청마다 3회다.
- 각 시스템 8개 조회 intent의 영어/한국어 요청 16개, 총 32개다. 이전 엔진 설계 fixture와 분리했다. 32개의 독립 업무나 대규모 품질 benchmark가 아니다.
- 정답은 사전에 지정한 **특정 method/path의 도구 포함률**이다. 동등한 결과를 내는 모든 대체 API를 인정한 업무 성공률이 아니다. 예를 들어 파일 내용은 contents와 raw API가 대안일 수 있다. 이후 선택 평가에는 허용 대체 도구를 사전에 명시해야 하며 이번 label·결과를 사후 교체하지 않았다.

```bash
uv run --no-project --no-config --no-env-file --isolated \
  --python 3.12 --with graph-tool-call==0.46.0 -- \
  python scripts/evaluate-tool-search.py --binary target/release/xgen \
  --output /tmp/xgen-tool-search-eval-new --freeze
```

새 경로에 새 manifest를 만들어 재실행한다. 기존 결과는 덮어쓰지 않는다. 이번 원본 결과·Run/Receipt·전체 그래프는 `/tmp/xgen-tool-search-eval-20261006-a`에 있고 공유 저장소에는 공개 입력·동결 정보·후보 ID·진단 요약을 보존했다. 원본 rows와 summary SHA256를 결과에 연결했다.

## 고정 검색어의 결과

| 시스템·언어 | 요청 수 | top-1 | top-5 | MRR@5 |
| --- | ---: | ---: | ---: | ---: |
| Gitea 영어 | 8 | 3/8 | 4/8 | 0.4375 |
| Gitea 한국어 | 8 | 0/8 | 2/8 | 0.1250 |
| WireMock 영어 | 8 | 6/8 | 8/8 | 0.8438 |
| WireMock 한국어 | 8 | 2/8 | 4/8 | 0.3229 |
| 전체 | 32 | 11/32 | 18/32 | 0.4323 |

기본 경로의 한국어→영어 명세 검색과 유사한 API 구분에 개선 여지가 있다. 영어에서도 Gitea의 정답 후보가 4개 요청에서 빠졌다. 검색기 직접 호출에서도 동일하게 빠지므로 **이번 고정 검색어의 누락은 xgen 후보 전달 손실로 설명되지 않는다.** 검색 설정·표현·색인·ranking을 graph-tool-call 쪽에서 평가해야 한다. 이 작은 endpoint 포함률을 기존 SEV 선택률과 직접 비교하거나 개선 효과라고 주장하지 않는다.

## 전달과 계약 검사

| 검사 | 결과 | 해석 |
| --- | --- | --- |
| 원본 직접 변환과 CLI import의 전체 normalized tool 정의 | 두 시스템 모두 일치 | xgen import 과정에서 추가 손실 없음. 완전한 원본 schema 보존의 증명과는 구분 |
| 직접 검색→CLI 후보 ID·순서·설명·score·producer | 32/32 요청, 3회씩 일치 | 후보 전달 결함 미검출 |
| CLI→다음 agent 모델 입력의 search output·exact request | 32/32 일치 | 모델에 후보를 전달하는 경로 정상 |
| 3회 반복 결과 안정성 | 32/32 안정 | 이 조건의 반복 안정성 확인 |
| 직접 tool→CLI describe의 full tool 정의 | 32/32 일치 | CLI 자체는 큰 정의도 반환 |
| agent 모델 입력의 describe·exact request | 18/32 일치, 14/32 실패 | 큰 정의를 한 observation으로 넘기는 연결 문제 |
| 사전 지정한 조회 도구의 필수 parameter | 32/32 보존 | 이번 대상의 필수 입력 누락 없음 |

14개 실패는 Gitea 9개·WireMock 5개에서 발생했다. 실패 사례의 CLI describe 출력은 **74,724~1,196,418 bytes**로 모두 agent observation 64 KiB 한도를 넘는다. 실제 저장한 Receipt를 확인했으며 search는 succeeded, describe는 failed다. 실패 output의 오류는 `tool_discovery_response_invalid`다. xgen이 한도를 넘는 결과를 성공으로 인정하거나 몰래 잘라 전달한 사례는 없다.

이 오류는 bound 검사에서 일반 응답 오류로 매핑된다. 사용자가 큰 명세 때문에 실패했음을 알 수 있도록 오류 표현도 개선해야 한다. 전체 크기만 늘리는 대신 **모델용 선택 정보와 호스트 실행용 완전한 계약을 분리하고, 필요한 계약을 digest로 연결해 단계적으로 조회하는 방식**을 우선 검토한다. 원본 스키마나 필수 입력을 잘라서 실행을 허용하는 방식은 채택하지 않는다.

## 정규화에서 추가 관찰한 문제

`source_description_missing=2`는 원본 정보 전체의 소실을 뜻하지 않는다. WireMock unmatched 요청의 두 언어 사례에서 normalized `tool.description`에 원본 상세 description이 없다는 검사 결과다. 원본 상세 문장은 `metadata.openapi.description`에 남아 있음을 별도로 확인했다. pinned parser는 `summary`가 있으면 `description`보다 우선한다. 검색용 표현이 필요한 구분 정보를 반영하는지는 upstream 색인·정규화 평가 대상이다.

또 원본 summary/description에 없는 `cluster-wide`가 Gitea 121/448개·WireMock 5/40개 도구 설명에 추가됐다. pinned parser의 `_enrich_description`은 path의 non-parameter segment가 3개 이상이고 GET/DELETE에 `{name}`·namespace placeholder가 없으면 이 표현을 붙인다. 이번 두 시스템에서 재현됐지만 검색 점수 저하의 원인이라는 인과 관계는 별도 ablation이 필요하다. 범용 OpenAPI에 도메인 가정을 붙이는 정규화 규칙은 **graph-tool-call 개선 과제**이며 도메인 adapter/configuration으로 범위를 제한하는 방향을 검토한다.

큰 describe의 metadata에는 `openapi`, `api_contract`, `response_schema`, `produces/consumes` 등 여러 표현이 함께 들어 있다. 예를 들어 `repoGetByID`의 전체 CLI 출력은 163,218 bytes다. full metadata의 반복 구조는 upstream의 선택용 compact view 개선 후보지만, 해당 표현을 그대로 모델 observation에 넣는 결정과 한도·오류 UX는 xgen의 책임이다.

## 시간·모델·비용

전체 반복의 중앙값은 direct search/producer 확장만 0.145초, artifact load+동일 검색 0.343초, CLI 전체 0.834초다. direct는 같은 Python process, CLI는 매번 새 process와 worker를 사용한다. 서로 측정 범위가 다르므로 차이를 검색 알고리즘 비용이나 순수 하네스 overhead로 확정하지 않는다. cache/worker 유지 최적화는 xgen의 별도 과제다.

agent 전달 검사는 결정적인 loopback endpoint 요청 82회로 수행했다. 이 endpoint는 실제 tool output을 다음 planningContext에서 읽고 관찰한 첫 후보를 describe했으며 gold label로 도구를 선택하지 않았다. 실제 LLM 정확도나 업무 API 성공률은 아니다.

사전 계획의 전달 합격 조건을 충족하지 못했으므로 **실제 DeepSeek 검색어 생성·후보 선택 단계는 실행하지 않았다.** 평가 대상 유료 LLM 호출과 업무 API 호출은 각각 0회다. 보조 delegate 코드 검토는 평가 대상 모델 호출·시간/비용 집계와 별개이며 해당 과금은 측정하지 않았다. 모델의 검색어 생성·SEV/LLM 선택 실패를 이번 결과로 판정하지 않는다. 기존 모델/프롬프트 기본값도 변경하지 않았다.

## 책임과 다음 순서

| 소유자 | 확정한 문제·남은 검증 | 다음 작업 |
| --- | --- | --- |
| xgen | 두 독립 시스템의 큰 describe observation 실패·모호한 오류 표시 | 모델용 명세 조회와 내부 실행 계약을 분리하는 일반 계약, 단계적 조회·digest 바인딩·한도 오류 표현 |
| graph-tool-call | 직접 검색의 목표 후보 누락, 원본에 없는 scope 설명 주입 | 범용 정규화와 검색 평가를 해당 저장소에서 개선. 사전 고정한 추가 사례에서 lexical/embedding/rerank 조건과 scope 보강 ablation 비교 |
| graph-tool-call | 큰 metadata의 여러 반복 표현 | 원본·실행 계약은 보존하면서 선택용 compact view를 제공할 수 있는 공개 계약 검토 |
| 모델·SEV | 이번 단계에서 미평가 | 명세 전달 합격 후 검색어 생성·후보 선택 평가. 검색 후보가 빠진 실패와 선택 실패를 구분 |

우선순위는 xgen의 큰 명세 연결 계약을 해결하고 같은 분리 평가를 재실행하는 것이다. 검색 개선은 graph-tool-call 저장소에서 독립적으로 진행하며 xgen 엔진에 업무별 검색 보정 규칙을 넣지 않는다. 현재 32개 요청은 이제 설계 자료로 취급하고, 개선 채택은 추가로 보류한 사례에서 검증한다.

검증: 새 평가 실행기 검사 5개 통과, 기존 모델 평가 실행기 검사 11개 통과·명시적인 example binary가 필요한 검사 1개 skipped, Python compile·공개 문서·diff whitespace 검사 통과. 런타임 Rust 코드는 변경하지 않아 Rust 전체 검사는 반복하지 않았다.
