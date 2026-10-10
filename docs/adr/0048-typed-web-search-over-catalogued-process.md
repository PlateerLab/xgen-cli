# ADR-0048: catalogued process 위의 typed web search

- 상태: 채택
- 날짜: 2026-10-05
- 범위: 일반 기능. 검색어·문서·도메인에 따른 engine 분기는 없다.

## 문제

검색 backend를 generic process 도구로 쓰면 모델이 backend argv, timeout, JSON 출력 옵션을 모두 구성해야 한다. 앞선 두 사례에서 영어 검색은 완료됐지만 한국어 검색은 실행 전 `invocation_invalid`로 거절됐다. 이 한 실패만으로 원인을 확정하지 않는다. typed 계약은 특정 사례 수정을 위해서가 아니라 host와 모델의 책임을 분리하기 위해 도입한다.

## 결정

`xgen.web/search@1.0.0` 입력은 `query`, `maxResults` 두 필드다. query는 비어 있거나 control character가 있으면 거절하며 최대 512 Unicode 문자·1024 UTF-8 bytes다. 결과 개수는 1–5다. host가 `openserp` 실행 파일을 catalogued한 경우만 capability를 제공한다. 대화형 `xgen`은 PATH에서 발견하고, headless는 기존 `--allow-executable openserp=/absolute/path`를 사용한다.

OpenSERP는 기존 shell-free process adapter로 실행한다. backend argv·30초 timeout·32768-byte 출력 한도는 host가 정한다. host가 private 임시 YAML 설정을 전달해 workspace 설정 파일을 읽지 않게 한다. 옵션 다음의 `--` 뒤에 provider와 query를 배치한다. 검색어를 shell이나 CLI 옵션으로 해석하지 않는다.

`web.search` scope의 resource는 정확한 query다. URL이나 hash로 바꾸지 않아 runtime normalization이 실제 검색어를 바꾸지 않는다. 검색어와 결과는 private run store에 남으므로 민감한 내용을 query에 넣지 않는 것은 호출자의 책임이다. 공개 evaluation에는 공개 문서 검색어만 기록한다.

검색 결과는 `outcome`, `query`, `results`, `exitCode`, `durationMs`로 정규화한다. 결과에는 `title`, `url`, `snippet`만 남긴다. 순서를 유지하고 URL syntax, credentials, field 크기와 전체 출력 크기를 검사한다. raw stderr와 host 경로는 출력하지 않는다. 실패·잘린 출력·잘못된 JSON은 빈 결과와 별도 outcome으로 남긴다. backend의 signal 종료도 관측된 실패로 처리한다. URL은 fetch하지 않는다. 이 검사는 source의 진실성·신뢰성·최신성을 보증하지 않는다.

검색은 외부 전송이므로 `DataBoundary::External`이다. catalog와 host 정책이 허용한 요청만 기존 execute 승인으로 실행한다. `NonIdempotent`, `Once`, durable tool output 및 Receipt를 사용한다. timeout 등의 알려진 실패는 결과로 저장하고, process adapter의 Unknown은 그대로 전달한다. 실행 여부가 불확실하면 재검색하지 않는다. 완료된 run의 replay는 저장된 최종 응답만 반환한다.

별도의 agent loop나 승인 runtime은 추가하지 않는다. process material provider를 재사용하되 web search recipe는 `xgen.cli.web-search-recipe/v1` domain으로 분리한다. capability definition·external instance·backend fingerprint는 execution profile에 포함된다. OpenSERP가 없는 기존 process profile의 serialization에는 새 field를 넣지 않는다.

## 검증과 남은 범위

영어·한국어 fixture에서 literal query, 설정 격리, malformed response, backend 실패를 확인한다. 승인 테스트는 execute 미승인·Once 이외 lifetime·backend 없는 요청을 검사한다. recipe reopen과 domain 분리, 전체 workspace 회귀 테스트를 수행한다.

실제 default model과 OpenSERP로 기존 영어·한국어 사례 2개와 별도 SQLite·Rust 사례 2개를 검증했다. 결과 및 한계는 [개발 기록](../development/typed-web-search-2026-10-05.md)에 남긴다. 4개 사례는 일반적인 검색 정확도나 장기간 가용성을 증명하지 않는다.

OpenSERP와 browser 설치 자동화는 아직 없다. 현재 native backend의 실제 검증 플랫폼은 Linux다. 장시간 terminal session, 입력 전달, cancellation은 다음 별도 작업이다.
