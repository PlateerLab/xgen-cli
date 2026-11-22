# Firecrawl Local 독립 실행 평가

2026-11-22, 일반 웹 읽기 도구 후보를 평가했다. **정적·JS 문서의 Markdown 추출 후보로 유지하되,
xgen 기본 도구 채택은 보류한다.** Runtime·prompt·배포본·기존 모델 설정은 변경하지 않았다.

## 실행 범위와 근거

- crates.io의 `firecrawl-local 0.1.1`을 `cargo install --version 0.1.1 --locked`로 임시 경로에 release
  빌드했다. 빌드 시간은 4분 34초다. 전역 PATH 등록·`init`·MCP 설정·계정 생성은 수행하지 않았다.
- Linux x86_64, 기존 Google Chrome 157.0.8103.78을 sandbox 기본 설정으로 사용했다.
- 별도 HOME·config·scratch, `FIRECRAWL_CLOUD=off`. 사용자 credential·브라우저 프로필을 전달하지 않았다.
- 정적 문서와 JS 렌더링 문서를 합성 설계 fixture로 만들고, 공개 Python CSV 문서와 W3C PDF를 보류
  입력으로 먼저 고정했다. 각 입력·경로는 3회씩 실행했다. 도구 구현을 사례에 맞춰 수정하지 않았다.
- HTTP 비교는 `urllib` + HTML visible text, PDF는 fetch + 기존 `pdftotext -layout`이다. 원본 HTML이나
  PDF bytes 자체와 Markdown의 길이·형식을 같은 것으로 비교하지 않는다.
- [원시 지표의 비민감 요약·artifact hash](2026-11-22-firecrawl-local-results.json)를 보존했다.
  Package VCS metadata는 `dirty=true`이므로 commit만으로 배포 crate의 정확한 내용을 주장하지 않는다.
  Published crate와 binary의 hash를 함께 기록한다.

공식 자료: [Local README](https://github.com/firecrawl/firecrawl/tree/main/apps/local),
[공식 발표](https://www.firecrawl.dev/changelog/firecrawl-local).
공개 입력: [Python csv](https://docs.python.org/3/library/csv.html),
[W3C PDF](https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf).

## 실제 결과

아래 성공은 사전 고정한 필수 문자열 보존 판정이다. 문서 전체의 의미적 정확성이나 LLM 과업 완료율이
아니다. 시간은 메모리 sampler를 분리한 CLI 실행의 중앙값이며, 두 경로를 수집한 시점이 달라 작은 차이를
일반적 성능 우위로 해석하지 않는다.

| 입력 | HTTP 비교 내용 보존 | Firecrawl 기본 auto 내용 보존 | Firecrawl 중앙 시간 |
| --- | --- | --- | --- |
| 정적 합성 문서 | 3/3 | 3/3 | 0.008초 |
| JS 합성 문서 | 0/3 | 3/3 | 0.784초 |
| 공개 Python CSV 문서 | 3/3 | 3/3 | 0.075초 |
| 공개 W3C PDF URL | 3/3 | 0/3, HTTP 403 차단 | 2.558초 |

- 정적·JS 합성 문서에서 표의 행·열과 Python fenced code, 본문을 보존하고 nav/footer를 제거했다.
- 공개 CSV 문서에서도 코드 블록과 필수 내용을 보존했다. Markdown 약29.5KB, 단순 텍스트 약22.3KB로,
  정리된 출력이 항상 더 짧거나 token 비용이 낮아지는 것은 아니다. Token 수·LLM 품질은 미측정이다.
- JS `--browser always` 별도 진단도 3/3이었다. 기본 auto의 성공을 이 결과로 대체하지 않았다.
- HTTP 비교가 받은 것과 같은 PDF bytes를 로컬 HTTP로 제공하면 Firecrawl도 3/3 추출했다. 공개 URL
  실패는 PDF parser 실패와 구분된다. 단순 1페이지 PDF 하나로 복잡한 PDF의 표·OCR 품질을 주장하지 않는다.
- 로컬 PDF는 실제 텍스트를 추출하면서도 scanned-image/no-OCR 경고를 반환했다. 이 경고의 판정 정확성은
  추가 입력에서 확인해야 하며, 이번 한 사례로 parser 수정 규칙을 만들지 않는다.
- Map은 local fixture 2개 URL을 반환했다. Crawl은 기본 시작 경로 범위에서 1개, `--entire-domain`을
  명시하면 3회 모두 2개 문서를 반환했다.

## 메모리와 오류 의미

- `serve` 시작 후 대기 상태 RSS는 **약17.4MiB**였다.
- 짧은 정적 실행은 GNU time max RSS **약21.1MiB**, 같은 로컬 PDF는 **약52.9MiB**였다.
- JS 실행은 Chrome을 포함한 프로세스 트리의 관측 peak RSS 합 중앙값이 **약1.22GiB**였다. 20ms 간격으로
  관찰한 RSS 합이라 shared page가 중복 계산될 수 있다. Private/PSS나 실제 독점 메모리 수치가 아니다.
- 실험 뒤 해당 임시 browser profile을 사용하는 Chrome 잔류 프로세스는 0개였다.
- 제작사의 11MB idle·37배 속도·230배 절감 수치는 재현한 것이 아니다. 기존 self-hosted 구성을 비교하지
  않았으며, 대기 RSS로 Chrome 실행 중 메모리를 추정해서는 안 된다.

**CLI exit 0 또는 `success=true`만으로 웹 읽기 성공을 판정할 수 없다.**
합성 HTTP 404는 error page를 추출하면서 exit 0·`success=true`·`metadata.statusCode=404`를 반환했다.
공개 PDF의 403 차단도 exit 0·`success=true`였으나 `blocked=true`, 빈 Markdown, warning이 있었다.
이는 scraping 호출 성공과 목표 페이지 접근 성공을 구분해야 한다는 근거다.

Cloud를 끈 search는 exit 1과 명시적 설명으로 거절했다. 시험용 Cloud URL trap의 요청은 0개였다.
이는 해당 실행 설정과 관찰 결과이며, 모든 외부 통신을 패킷 단위로 감사했다는 뜻은 아니다.

## 측정 오류와 한계

초기 HTTP 비교기는 inline 태그 사이에 임의 공백을 넣어 `csv.reader`를 다르게 표현했다. 일반 DOM text
처리로 고치고 inline text·script 제외를 확인한 뒤 HTTP 경로를 다시 측정했다. 원래 결과는 보존했다.
초기 20ms 메모리 sampler는 매우 짧은 실행을 놓쳤고, sampler 종료 대기가 wall time에 섞였다. RSS 0은
미관측으로 처리하고, 짧은 실행은 GNU time으로 보완하고 시간을 별도 수집했다.
시간 수집 v2의 HTTP child에서 디렉터리 초기화가 실패한 12개 기록은 도구 실패로 집계하지 않았다.
원본을 보존하고 child 초기화를 수정한 v3에서 HTTP 경로 12개를 다시 수집했다.

이 수정 과정에서 보류 입력을 보았으므로 이번 결과를 sealed holdout 성능 증명으로 사용하지 않는다.
측정기 오류 수정과 도구 변경을 구분한 소규모 실행 가능성 조사다. 합성 입력 2개·공개 입력 2개·각3회이며,
범용 웹 성공률·모델 token 절감·xgen 작업 완료율·운영 안정성은 확인하지 않았다.

## xgen에 적용할 때의 조건

1. 일반: 선택 가능한 웹 읽기 backend 후보로만 검토한다. 기존 HTTP/API 조회나 browser-use의 화면 조작
   역할을 한 번에 교체하지 않는다.
2. 일반: `statusCode`, `blocked`, 빈 본문, warning, 결과 출처를 호스트와 모델에 보존한다. Receipt의 실행
   성공과 원격 페이지 내용 확보를 구분하고 exit code만으로 성공 처리하지 않는다.
3. 일반: Cloud 사용은 별도 명시 설정으로 두고 기본 로컬 모드는 `off`를 유지한다. 신뢰하지 않는 URL은
   공식 `--public-only` 경계를 사용한다. 이번 조사는 redirect·DNS·browser subresource 방어의 감사가 아니다.
4. 일반: JS 렌더링의 동시 실행·시간·메모리·출력 크기 상한과 중단 후 정리를 설계하고 검증한다.
5. 일반: 현재 crate의 AGPL-3.0 표기를 기록하고, 실제 연결·배포 방식을 정하기 전에 해당 방식을 검토한다.
   이번에는 xgen 의존성 추가나 재배포를 하지 않았다.

다음 단계는 위 조건을 만족하는 작은 adapter 설계와 별도 입력에서의 검증이다. 이번 실행만으로 기본
backend 채택이나 xgen runtime 계약 변경을 승인한 것으로 간주하지 않는다.
