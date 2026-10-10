# Typed web search 구현 및 검증

일반 기능: 모델이 검색어·결과 개수만 넘기는 `xgen.web/search@1.0.0`을 등록했다. 상세 계약은 [ADR-0048](../adr/0048-typed-web-search-over-catalogued-process.md)에 있다.

## 사용

OpenSERP가 PATH에 설치돼 있으면 기존 모델 설정으로 `xgen`을 실행하고 검색을 요청하면 된다. 검색 실행에는 기존 execute 승인 화면이 나온다. 검색 provider나 argv를 사용자가 매번 입력하지 않는다.

Headless에서는 다음처럼 기존 executable catalog로 backend를 지정한다.

```bash
xgen run --workspace . --allow-dir . \
  --allow-executable openserp=/absolute/path/openserp \
  --allow-remote-model-egress --allow-read --allow-execute \
  '웹에서 Rust 공식 문서를 찾아줘'
```

OpenSERP v0.8.12와 browser가 필요하다. 설치 자동화는 별도 후속 작업이다. 기본 executable catalog에 없는 binary를 모델이 임의로 설치하거나 실행하지 않는다.

## 실제 검증

[공개 결과](../../evals/tool-backends/results/2026-10-05/typed-web-search.json)는 Linux에서 default model profile과 OpenSERP를 사용한 결과다. credentials와 private SQLite journal은 포함하지 않았다. 자동 retry는 없다. [초기 검증 결과](../../evals/tool-backends/results/2026-10-05/typed-web-search-initial.json)도 별도로 보존했다. 최초 4건과 최종 binary의 반복 4건이 모두 통과했다.

| 구분 | 검색어 | 승인·최종 응답·offline replay |
| --- | --- | --- |
| 기존 사례 | curl CURLOPT_SSL_VERIFYPEER official documentation | 통과 |
| 기존 사례 | 파이썬 ast 공식 문서 | 통과 |
| 별도 검증 사례 | SQLite WAL official documentation | 통과 |
| 별도 검증 사례 | Rust 소유권 공식 문서 | 통과 |

최종 검증에서는 저장된 query가 요청 query와 정확히 같은지도 확인했다. 각 run은 승인 전 검색 output 0개, 승인 후 typed 검색 output 1개와 Receipt 1개를 기록했다. generic process invocation은 0개였다. 최종 응답의 두 제목·URL은 관측 결과에서 정확한 순서로 복사됐고, 완료 후 잘못된 model endpoint로 재개해도 같은 응답을 반환했다. replay 전후 journal과 usage DB digest가 같았다. Unknown과 중복 effect start는 없었다.

검색 엔진 결과는 외부 환경에 따라 달라진다. 이번 검증은 작업 경로와 응답의 observation 일치를 확인한 것으로, source 내용의 진실성이나 모든 검색어의 성공을 보장하지 않는다. 이전 generic process 사례의 실패 원인을 확정하거나, 4/4를 일반적인 성능 향상률로 해석하지 않는다.

## 재현

```bash
python3 scripts/smoke-web-search.py \
  --binary target/release/xgen --openserp /absolute/path/openserp \
  --root /private/new/evaluation-directory
```

Live runner는 model 요청과 외부 검색을 수행한다. 공개 CI는 offline fixture·승인·recipe·protocol 테스트만 수행한다. 두 smoke runner는 같은 승인·Receipt·replay 검사를 사용한다.

Offline 검증은 전체 Rust 643개, Python 42개, npm 14개 테스트를 통과했다. 최종 profile descriptor 변경 후 CLI unit 66개를 다시 검사했다. workspace lint, format, release build, license notices 및 public documentation·release workflow 계약 검사도 통과했다.
