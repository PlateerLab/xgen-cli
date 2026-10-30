# ADR-0080: 상태 폴더 권한 거절은 로컬 복구 정보를 제공한다

- 상태: Accepted
- 기준일: 2026-10-31
- 분류: 범용 CLI·REPL 오류 진단. 모델 입력·journal·envelope·파일 접근 승인 계약 변경 없음.
- 후속 결정: ADR-0023의 상태 폴더 검사와 로컬 오류 표시.

## 문제

Unix의 `XGEN_STATE_HOME`이 0775이면 기존 group/other 접근 거절 검사는 작동하지만,
새 Run에서는 `internal_safety_failure`, 기존 Run 조회에서는 `configuration_mismatch`로 바뀌었다.
사용자는 문제 경로나 권한, 복구 방법을 알 수 없었다. 상태 root뿐 아니라 `runs` 폴더와
개별 Run 폴더의 동일 검사에서도 정보가 사라졌다.

## 결정

1. 기존 `mode & 0o077 != 0` 거절 조건은 그대로 둔다. 해당 경우만 typed permission 오류로
   경로·관찰한 mode를 전달한다. 파일 생성·읽기·chmod·symlink·경로 경계 검사 순서를 바꾸지 않는다.
2. 공개 code는 `state_directory_permissions`, exit code는 사용자 구성 오류인 64로 한다.
   기존 root/runs 생성 경로의 이 실패는 70에서 64로 바뀐다. 그 밖의 실패 분류는 유지한다.
3. `run`·`resume`·`usage`·`recover`와 REPL에 같은 진단을 제공한다.
   상태 조회·변경 내역 조회도 같은 typed 오류를 보존한다. headless stdout에는 진단을 넣지 않는다.
4. 명시적인 로컬 진단에서만 다음을 보여 준다.

   ```text
   XGEN_ERROR code=state_directory_permissions
     state_directory="/example/private-state"
     permissions=0775; required=no group or other access
     recovery=Check the quoted directory, then remove group/other access with chmod go-rwx -- PATH. Permissions were not changed automatically.
   ```

   `PATH`에는 확인한 실제 폴더 경로를 전달한다. 표시된 문자열을 shell 명령으로 자동 실행하지 않는다.
   기존 검사는 0700만 허용하는 검사가 아니므로 안내도 group/other 접근 제거를 요구한다.
   `chmod go-rwx`는 소유자 권한을 유지한다. 폴더와 자식의 권한을 자동 변경하거나 재귀적으로 고치지 않는다.
5. 경로는 Rust path Debug의 인용·escape로 표시한다. 개행·ESC·비 UTF-8 경로도 terminal control로
   실행되지 않는다. 오류 payload의 일반 Debug와 RunLayoutError Display는 경로를 가린다.
   ADR-0023의 일반 오류에 경로를 싣지 않는 원칙에 대해 이 로컬 상태 폴더 복구 정보만 명시적으로 추가한다.
   내부 OS error chain, 파일 내용, 모델 응답, endpoint, credential은 표시하지 않는다.
   경로를 journal·모델 context·progress event·telemetry에 넣지 않는다.
6. symlink·없는 폴더·잘못된 state root·일반 OS I/O 오류에는 권한 문제라고 추정하거나
   `chmod`를 권하지 않는다. Windows의 기존 검사·오류 동작도 유지한다.

## 검증

- 설계 사례: 상태 root 0775와 `runs` 0755. CLI `usage`·`recover`는 정확한 폴더·mode와
  exit 64를 보이며 stdout이 비어 있고 기존 권한은 그대로다. DB도 생성하지 않는다.
- 별도 검증 사례: 개별 Run 폴더 0770, space가 있는 경로, symlink 대상 0775.
  symlink는 기존 generic 오류를 유지하고 target의 mode와 경로를 노출하거나 고치지 않는다.
- 사용자가 직접 문제 폴더를 0700으로 바꾸면 permission 거절을 통과해 다음 단계인 DB 무결성 검사로
  진행한다. fixture에는 DB가 없으므로 integrity 오류가 나며 DB는 여전히 생성되지 않는다.
- 실제 REPL child process에서도 새 목표가 typed 오류와 복구 안내를 표시한다. 외부 모델 호출은 없다.
- 개행·ESC·비 UTF-8 경로의 진단은 escape되고, 오류 Debug·Display에는 경로 sentinel이 없다.
- 기존 permissive root 검사도 typed 오류를 기대하도록 바꾸고 자동 chmod 금지 assertion은 유지했다.

## 범위

소유자는 xgen CLI의 RunLayout 오류 전달·표시다. filesystem adapter, 외부 도구, 모델의 문제는 아니다.
Unix mode 비트 검사의 설명을 개선했으며 ACL·소유자 UID 검사나 Windows ACL 정책을 새로 도입하지 않는다.

로컬 전체 회귀는 Rust 778 passed·4 ignored(47 suites), Clippy·fmt·protocol·공개 문서·배포 workflow 검사 통과다.

변경 추적: [PR #137](https://github.com/PlateerLab/xgen-cli/pull/137), 코드 `7edf540`,
[5개 플랫폼을 포함한 전체 CI](https://github.com/PlateerLab/xgen-cli/actions/runs/41077456833).
