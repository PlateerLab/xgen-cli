# ADR-0053: 큰 UTF-8 파일의 줄 범위 읽기

- 상태: Accepted
- 기준일: 2026-10-25
- 변경 분류: 범용 filesystem capability 추가. 특정 언어·파일 형식·업무 규칙 없음.
- 기존 경계: ADR-0022, ADR-0024, ADR-0052
- protocol 변경: 새 CapabilityDefinition fixture 하나. 기존 Definition·schema·journal·SQLite schema는 그대로다.
- planner prompt 변경: 없음. 새 capability는 planning context의 capability 목록으로만 전달된다.

## 문제

`xgeny.fs/read-text`는 64 KiB를 넘는 파일을 거절한다(ADR-0022). ADR-0022는 상한을 높이지 않고 후속
range/chunk capability로 다루기로 했다. 실사용 시나리오에서 75 KB Python 파일 하나가 이 상한에
걸렸다. ADR-0052 이후 요청은 더 이상 `failed_work`로 끝나지 않지만, 모델은 그 파일을 읽을 방법이
없어 같은 실패 읽기와 검색을 반복하다 턴 예산을 소진했다. 소스 코드, 로그, 생성된 문서처럼 64 KiB를
넘는 텍스트 파일은 흔하다.

## 결정

### 1. `xgeny.fs/read-text-lines@1.0.0`를 추가한다

입력은 `{path, startLine, lineCount}`다. `startLine`은 1부터 세고, `lineCount`는 1–2,000이다. 추가
필드는 거절한다. `--allow-dir`·`--allow-file` 범위는 `read-text`와 같은 규칙(exact file 또는 허용
directory의 하위)으로 검사한다. effect class는 `read_only`이고 `--allow-read` 승인 아래 실행된다.

출력은 `{content, startLine, endLine, totalLines, truncated, digest}`다.

- 줄은 `\n` 기준으로 나누고, 각 줄은 원래 종결자(`\n` 또는 `\r\n`)를 그대로 포함한다. 이어지는 범위를
  붙이면 원래 파일과 바이트 단위로 같다. 마지막 줄 뒤의 종결자는 빈 줄을 만들지 않는다.
- `endLine`은 마지막으로 반환한 줄이다. 줄을 하나도 반환하지 않으면(범위가 파일 끝을 넘는 경우)
  `startLine - 1`이다. `totalLines`로 모델이 다음 범위를 고른다.
- `digest`는 다른 workspace 조회(list-directory·stat·search-text)와 같이 digest를 뺀 출력의 RFC 8785
  canonical 바이트에 대한 SHA-256이다. 파일 내용 digest가 아니므로 `apply-patch`의 `expectedDigest`로
  쓸 수 없다.

### 2. 크기 상한

- 파일은 8 MiB까지 읽는다. 더 크면 확정 실패다. search-text의 전체 스캔 상한과 같은 값이다.
- 파일 전체가 UTF-8이어야 한다. 아니면 확정 실패다.
- 반환하는 `content`는 JSON escaping 뒤 64 KiB 이하다. raw 바이트가 아니라 escape된 크기로 세므로
  control 문자가 많은 텍스트에서도 출력 하나가 ADR-0022가 계산한 PlanningContext 여유 안에 머문다.
- 상한에 닿으면 그 앞의 온전한 줄까지만 반환하고 `truncated: true`다. 요청 범위의 첫 줄 하나만으로
  상한을 넘으면 그 줄을 문자 경계에서 잘라 반환하고 역시 `truncated: true`다. 이때 그 줄의 나머지는
  이 capability로 읽을 수 없다. minified 파일처럼 한 줄이 64 KiB를 넘는 경우의 한계로 남긴다.

### 3. 실행 의미

경로는 `read-text`·search-text와 같이 symlink를 따르지 않는다. 실패는 기존 조회 adapter와 같은 고정
분류 digest(`open`, `read`, `limit`, `decode`, `encode`)만 남긴다. 실패는 ADR-0052에 따라 recoverable
read failure가 된다. 원인 분류 code는 [ADR-0054](0054-value-free-step-failure-codes.md)에 따라 전달된다. verifier는 digest와 함께 줄 번호 관계
(`endLine + 1 = startLine + 반환 줄 수`, `endLine <= totalLines`)와 content 상한을 다시 검사한다.

## 대안

- **read-text 상한 상향:** 출력 하나가 PlanningContext 상한을 위협한다(ADR-0022). 거절했다.
- **byte offset 범위:** UTF-8 경계와 줄 경계를 모델이 맞춰야 한다. search-text가 줄 번호를 주므로
  줄 범위가 바로 이어 쓰기 좋다.
- **read-text에 선택 필드 추가:** 기존 Definition은 immutable 계약이고 digest가 저장된 Run에 묶여 있다.
  새 contract로 분리했다.

## 결과

- 64 KiB를 넘고 8 MiB 이하인 UTF-8 파일을 범위로 나눠 읽을 수 있다.
- workspace discovery mode의 capability가 6개에서 7개가 된다. workspace discovery profile digest가
  바뀌므로 이 변경 이전 binary로 시작해 멈춘 workspace Run은 새 binary에서 `ConfigurationMismatch`로
  재개가 거절된다. 완료된 Run의 offline replay는 영향이 없다.
- 큰 파일 수정은 여전히 불가능하다. `apply-patch`·`write-atomic`도 64 KiB 상한이다. 별도 결정이 필요하다.
  [ADR-0056](0056-large-file-patch.md)에서 `apply-patch` 대상을 8 MiB로 올리고 `fileDigest`를 더했다.
- 같은 파일을 여러 범위로 읽으면 tool output이 planning context에 쌓인다. context budget 소진은 기존
  `ContextBudgetExceeded` 경로를 탄다.

## 검증

- adapter: 범위 연결이 원본과 같음, CRLF 보존, 파일 끝을 넘는 범위, 빈 파일, escape 기준 상한 절단,
  첫 줄 절단, 8 MiB 초과·비 UTF-8·directory 거절, symlink 비추적, 입력 shape 거절, 출력 변조 거절.
- 권한: exact file과 허용 directory 하위만 승인하고 범위 밖 인자를 거절한다.
- e2e: 64 KiB를 넘는 파일의 끝 범위를 public `xgen run`으로 읽고 다음 planning context에서 출력을 본다.
- 실사용(대화형 `xgen`, 같은 planner model, 2026-10-25):
  - 설계 사례: 75 KB·2,004줄 `server.py`가 있는 Python 저장소에서 README의 도구 목록 대조. 이전에는
    턴 예산 소진으로 끝났다. 이번에는 1–1,655줄(content 상한으로 `truncated`)과 1,656–2,004줄을 이어
    읽고 18 Step·51초에 완료했다. 다만 모델의 README 수정(중복 행 추가)과 완료 요약이 실제 변경과
    달랐다. capability가 아니라 planner 출력 품질 문제로 따로 다룬다.
  - 검증 사례(설계에 쓰지 않은 저장소): 113 KB·3,296줄 HTTP executor의 기본 timeout과 HTTP 오류 처리
    설명. `read-text`가 두 번 실패한 뒤 search-text·stat으로 위치를 찾고 100–200줄, 360–479줄을 읽어
    6 Step·11초에 줄 번호까지 정확히 답했다.
  - 두 사례 모두 같은 실패 읽기를 반복했다. 실패 원인 분류를 모델에 전달하지 않는 한계(ADR-0052)다.
