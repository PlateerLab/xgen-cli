# 직접 만든 작업 8개로 xgen 중간 점검

- 기준일: 2026-10-31
- 상태: 과제 사전 검증 8/8 통과, 실제 모델 실행은 인증 오류로 미실행.
- 성격: 실제 업무를 본뜬 합성 과제다. 사용자 실제 저장소 8개를 성공한 결과가 아니다.
- 범위: 평가 fixture와 기록만 추가하며 xgen 엔진·프롬프트·기본값은 바꾸지 않는다.

## 과제

| 과제 | 확인할 작업 | 실행 방식 |
| --- | --- | --- |
| pagination | 마지막 페이지와 경계 입력의 cursor 버그 수정 | REPL |
| config-precedence | false·0·빈 문자열을 잃는 설정 병합 수정 | run → execute 승인 대기 → resume |
| csv-export | 쉼표·따옴표·개행·Unicode CSV 출력 수정 | REPL |
| cli-delimiter | `--` 뒤의 옵션 모양 파일명 처리 | run → execute 승인 대기 → resume |
| multi-file-rename | 모델·출력·호출부 세 파일의 필드 이름 변경 | REPL |
| safe-relative-path | 경로 정규화 수정과 AGENTS.md 지침 준수 | REPL |
| docs-cli-audit | CLI 구현·help를 조사해 README만 수정 | REPL |
| git-worktree-discovery | `.git`이 파일인 worktree도 탐색 | REPL |

각 과제는 외부 패키지가 필요 없는 Python·Node 프로젝트다. 테스트·설정·지침 파일은 보호하고
지정한 수정 대상만 바꾸도록 요청한다. 숨김 테스트는 모델 workspace에 넣지 않고 별도 validation copy에서
실행한다. `oracle-fixes.json`은 과제 검증용 정답이며 모델 workspace에 복사하지 않는다.

## 사전 고정과 판정

- 각 과제를 새 workspace·state에서 두 번씩, 총 16회 실행한다. 실패를 지우거나 성공할 때까지 재시도하지 않는다.
- 기존 `scripts/evaluate-projects.py`를 그대로 사용한다. 현재 두 실행 방식 모두 24 model-turn budget이며
  run/resume은 invocation당 256 ticks다. 개별 subprocess timeout은 240초다.
- 완료 candidate뿐 아니라 독립 공개·숨김 테스트, 빌드, 보호 파일 보존, 완료의 offline replay와
  journal/usage 불변성을 확인한다. run/resume은 execute 승인 전 실행이 없었는지도 확인한다.
- 호출 수·tool effect 수·Unknown·중복 effect 시작·소요 시간·입출력 token과 cache usage를 기록한다.
  같은 명령을 서로 다른 step으로 반복한 수와 자연어 답변의 사실성 전체는 기존 runner가 자동 판정하지 않는다.
  필요하면 해당 Run의 recipe·output·최종 답변을 별도 검토한다.
- safe-relative-path의 `CHECKPOINT:` 답변 접두 규칙은 최종 답변과 비교해 별도로 확인한다.
  기존 runner의 기능 pass에는 이 접두 규칙 판정이 포함되지 않는다.
- 첫 네 과제는 design, 뒤 네 과제는 held_out으로 표시했다. 이번은 수정 전 기준점 측정이며 A/B 개선 주장은 없다.
  모두 결과를 분석한 뒤에는 새로운 엔진 변경을 검증할 untouched holdout으로 재사용하지 않는다.
- 이 실행은 pipe REPL·headless 실행 점검이다. 실제 TUI 키 입력·화면 렌더링은 별도
  `scripts/smoke-inline-tui.py` 검사와 구분한다.

## 실행

과제만 검증하며 모델 API를 호출하지 않는다.

```bash
python3 evals/created-tasks-2026-10-31/check.py
```

원본 실패, 정답 수정 뒤 공개·숨김 테스트/빌드 통과, 보호 파일 보존을 확인한다.
로그·사전 등록은 새 0700 임시 폴더에 남긴다. binary·cases·oracle·runner hash를 먼저 기록한다.

실제 모델로 실행하려면 로컬 터미널에서 `xgen model setup`으로 활성 profile의 인증을 설정한 뒤 실행한다.
인증 정보는 채팅이나 저장소에 넣지 않는다. 기존 runner는 API 환경 override를 제거하므로
이번 실행도 같은 환경 규칙으로 profile 검사를 한다.

```bash
python3 evals/created-tasks-2026-10-31/check.py --execute
```

모델 검사가 실패하면 작업을 시작하지 않고 실패 code와 모델 호출 0회를 기록한다.
모델 검사는 catalog GET이며 실제 planning 호출과 구분한다.
모델 검사가 통과하면 기존 evaluator가 별도 `REPORT_ROOT` 아래 실제 실행 결과를 남긴다.

## 이번에 확인한 결과

[사전 검증 기록](preflight.json)의 8개 과제 모두 원본 실패·정답 수정 뒤 공개/숨김 테스트·빌드·보호 파일
검사가 통과했다. 실제 실행 gate는 `authentication_rejected`로 닫혔다. 별도의 default profile 검사도
`credential_not_found`였다. 이 환경에는 해당 모델용 API key 환경변수가 없었다.

실제 모델의 작업 완료율·반복 행동·token 사용량·모델 응답 시간은 아직 측정하지 않았다.
사전 검증의 8/8을 xgen의 성공률로 해석하지 않는다.
