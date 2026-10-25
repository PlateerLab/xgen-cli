# ADR-0056: 64 KiB를 넘는 파일의 patch

- 상태: Accepted
- 기준일: 2026-10-26
- 변경 분류: 범용 capability 계약. 특정 도구·파일·업무 규칙 없음.
- 기존 경계: ADR-0022, ADR-0025, ADR-0026, ADR-0053, ADR-0055
- 계약 변경: `xgeny.fs/apply-patch` 1.0.0 → 2.0.0, `xgeny.fs/read-text-lines` 1.0.0 → 2.0.0.
  protocol·SQLite schema와 journal 이벤트 형식은 그대로다.

## 문제

ADR-0053으로 64 KiB를 넘는 파일을 줄 범위로 읽게 됐지만 고칠 수는 없었다. 두 가지가 막았다.

1. `apply-patch`는 대상 파일 자체를 64 KiB까지만 읽었다. 출력 schema의 `byteSize`도 65536이 상한이었다.
2. 큰 파일의 `expectedDigest`를 얻을 길이 없었다. `read-text`는 64 KiB를 넘으면 실패하고,
   `read-text-lines`의 `digest`는 출력에 대한 digest라 파일 digest로 쓸 수 없다.

그래서 75 KB, 113 KB 소스 파일의 한 줄 수정 요청이 `too_large`로 끝났다.

## 결정

### 1. `read-text-lines@2.0.0`은 파일 전체 digest를 함께 낸다

출력에 `fileDigest`를 더한다. 읽은 파일 전체 바이트의 SHA-256이고, `read-text`의 `digest`와 같은
값이다. 범위를 읽은 뒤 바로 `apply-patch`의 `expectedDigest`로 쓴다. 기존 출력 digest(`digest`)의 뜻은
그대로다. verifier는 `fileDigest`의 형식을 검사하고, 출력 digest가 이 필드까지 포함해 맞는지 확인한다.

### 2. `apply-patch@2.0.0`은 8 MiB 파일까지 고친다

- 대상 파일과 결과 파일은 각각 8 MiB까지다. `read-text-lines`와 search-text 전체 스캔 상한과 같은
  값이다. 범위로 읽을 수 있는 파일은 고칠 수 있다.
- edit 인자의 상한(oldText·newText 합계 64 KiB, edit 32개)은 그대로다. journal과 planning context에
  남는 것은 이 인자와 path·digest·byteSize·changed·editCount뿐이고 파일 내용은 들어가지 않는다.
  그래서 ADR-0022가 계산한 출력·context 상한 근거는 바뀌지 않는다.
- commit 직전의 precondition 확인과 verifier의 postcondition 확인도 8 MiB까지 읽는다.
- 8 MiB를 넘는 파일은 전과 같이 `too_large`로 확정 실패하고 Run을 멈춘다(ADR-0055).

`write-atomic`은 64 KiB 그대로다. 내용 전체가 인자로 journal에 남고, 큰 파일을 통째로 새로 쓰는
사례는 관찰되지 않았다. 다만 commit의 기존 파일 확인 상한이 8 MiB로 같이 올라가므로, 이제
64 KiB 이하 내용으로 8 MiB 이하 기존 파일을 교체할 수 있다.

### 3. planner 안내

workspace 범위 안내문이 `apply-patch`의 digest를 "read-text `digest` 또는 read-text-lines
`fileDigest`"로 명시한다. 두 capability의 summary도 같은 관계를 적는다.

## 결과

- 64 KiB를 넘고 8 MiB 이하인 UTF-8 파일을 범위로 읽고 고칠 수 있다.
- 큰 파일을 고칠 때 메모리에 최대 8 MiB를 몇 번 읽고 해시한다.
- 두 Definition의 digest가 바뀌어 workspace profile digest가 바뀐다. 이 변경 이전 binary로 시작해
  멈춘 workspace Run은 새 binary에서 `ConfigurationMismatch`로 재개가 거절된다. 완료된 Run의 offline
  replay는 영향이 없다(ADR-0053과 같음).

## 검증

- adapter: 128 KiB 파일 patch가 적용되고 결과 digest·byteSize가 맞다. 8 MiB를 1바이트 넘는 파일은
  `too_large`로 실패하고 파일은 그대로다. 결과 크기 경계는 8 MiB에서 확인한다. `read-text-lines`
  출력의 `fileDigest`는 파일 전체 digest와 같다.
- e2e: 64 KiB를 넘는 파일의 끝 범위를 `read-text-lines`로 읽은 뒤 다음 planning context의
  `fileDigest`가 파일 digest와 같다. 이어서 그 digest로 낸 patch가 적용되고 Run이 exit 0으로 끝난다.
  8 MiB를 넘는 파일의 patch는 `XGEN_STEP_FAILURE ... code=too_large`로 멈춘다.
- 실사용: 대화형 `xgen`, deepseek profile, 이전 main(eeafb50)과 이 변경을 사례마다 3회씩 실행했다.

  | 사례 | 이전 | 이후 |
  | --- | --- | --- |
  | graph-tool-call, 113 KB `http_executor.py`의 함수 한 줄 수정 (설계) | 3회 모두 `too_large`로 멈춤 | 3회 모두 완료, 4단계, 10~12초, 정확한 한 줄 |
  | ku-portal-mcp, 75 KB `server.py`의 기본값과 docstring 두 곳 (설계) | 3회 모두 `too_large` | 3회 모두 완료, 4단계, 12초, 두 곳 정확 |
  | 다른 Python 서비스 저장소, 97 KB `main.py`의 포트 두 곳 (검증용, 설계에 안 씀) | 3회 모두 `too_large` | 3회 모두 완료, 4단계, 10~13초, 두 곳 정확 |

  이후 binary의 단계는 매번 `read-text` 실패(`too_large`), search-text, `read-text-lines`,
  `apply-patch`였다.

  회귀 확인:
  - 작은 파일 수정 중 사용자 동시 편집(ADR-0055 사례): 2회 모두 `precondition_conflict` 뒤 다시 읽고
    완료했다. 사용자 수정이 보존됐다.
  - 9 MB 파일의 마지막 줄 수정: `read-text-lines`가 `too_large`로 실패했다. 한 번은 Run이 거절됐고,
    한 번은 모델이 별도 승인된 process 실행으로 고쳤다. `apply-patch` 경로는 열리지 않았다.
