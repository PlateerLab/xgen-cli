# 로컬 빌드 프로젝트 검증

일반적인 workflow 평가다. Rust Cargo, Node.js built-in test, Python unittest 프로젝트에서 실패 수정,
기존 테스트·설정 보존, 추가 경계 검증, 승인 pause 뒤 process 재시작 resume, 완료 응답 offline replay와
usage 보존을 확인한다. 업무 프로젝트나 공개 게시 artifact의 설치 파일럿이 아니다.

```bash
python3 scripts/evaluate-projects.py --binary target/release/xgen
```

실제 API 호출이 발생한다. active 모델 프로필과 OS credential store를 사용하고 API 관련 환경 override는
제거한다. Binary/cases/runner hash를 실행 전에 고정하고 design 세 사례를 먼저, held_out 세 사례를 나중에
각 두 번 실행한다. 실패 실행은 삭제하거나 성공할 때까지 재실행하지 않는다.

Rust는 기본 bare REPL의 8 model-turn budget, Node.js·Python은 사전 고정한 24 model turns/256 ticks를
사용한다. Node.js·Python의 첫 invocation에서는 execute를 승인하지 않고 exact Run을 새 process에서
재개한다. 모드와 budget이 달라 언어별 성능 비교로 해석하지 않는다.

Fixture는 외부 package를 설치하지 않는 독립 프로젝트다. Model에는 source와 visible tests만 제공하고,
별도 validation copy에서만 hidden tests를 추가한다. 숨겨진 입력은 기능적 경계 탐색이며 visible 명세에
없는 요구까지 포함할 수 있다. 실패를 곧바로 engine 결함으로 해석하지 않는다.

`pass`는 완료 candidate, 기존 파일 보존, 독립 test/build·hidden tests, 완료 offline replay와 journal/usage
불변성 및 run-resume의 execute pause를 만족했다는 뜻이다. 실패 process 관찰과 최종 코드 원문 인용은
별도 지표이며 pass가 답변의 사실성 전체를 보장하지 않는다. 코드 인용은 outer whitespace만 무시한다.
Unknown과 중복 effect starts는 journal에서 별도로 검사한다. process tree leak은 이 runner가 측정하지 않는다.

원본 state/workspace는 0700 private 임시 root에 보존한다. 공유 결과에는 원문 goal, 응답, source diff,
endpoint, credential, tool stdout/stderr, Run ID를 포함하지 않는다. Fixture catalog 자체는 공개 예제다.
발표/배포용 사용자 검증은 별도의 RC3 Developer Preview 파일럿 절차를 따른다.

[첫 12회 결과](results/2026-10-04/report.json), [독립 안전성 재검사](results/2026-10-04/safety-audit.json),
[수동 검토](results/2026-10-04/review.json), [실행 시점 runner snapshot](results/2026-10-04/runner-at-execution.py)를 보존한다.
측정 뒤 Unknown 이름 오류와 timeout 부분 관측을 보완했다. 원시 측정 결과는 그대로이며 현재 runner로
API 실행을 반복해 결과를 개선하지 않았다. 이미 분석한 held_out은 다음 후보의 untouched 검증에 재사용하지 않는다.
