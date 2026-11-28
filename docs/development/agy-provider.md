# 실험용 agy 모델 연결

Linux 개발 빌드에서 공식 agy CLI의 기존 로그인을 사용한다. Python bridge나 별도 서버는 필요 없다.
rc.9부터 Linux 배포본에 실험 기능으로 포함되며 rc.8 이하에는 없다. [ADR-0090](../adr/0090-experimental-agy-provider.md)을 따른다.

## 연결

설치된 공식 agy에서 로컬 로그인을 마친 뒤 실행한다. 인증값은 xgen 설정이나 채팅에 넣지 않는다.

agy를 쓰는 빌드와 rc.8 이하가 같은 설정 파일을 공유하지 않도록 별도 폴더를 지정한다. Agy 프로필이 들어간 설정은
format2이며 rc.8 이하는 읽지 못한다. 아래 명령은 저장소 루트에서 실행한다.

```bash
export XGEN_CONFIG_HOME="$HOME/.config/xgen-agy-preview"
./target/debug/xgen model setup --provider agy --name agy
./target/debug/xgen
```

기본 모델은 `gemini-3.8-flash-low`다. 기존 API 프로필과 다른 이름을 사용하면 두 연결을 유지할 수 있다.
Setup은 catalog와 exact model을 확인한 뒤 새 프로필을 활성화한다. 아직 inference 호환성을 검사했다고
표시하지 않는다. 명시적으로 한 번 확인하려면 다음 명령을 쓴다. 이 검사는 모델을 호출한다.

```bash
./target/debug/xgen model check --profile agy --compatibility
```

다른 위치의 CLI나 timeout을 지정할 수도 있다.

```bash
./target/debug/xgen model setup --provider agy --name agy \
  --agy-executable /absolute/path/to/agy --inference-timeout 60
```

SSH 환경변수는 agy 자식에 전달하지 않는다. 부모 SSH 연결은 바꾸지 않는다. 직접 CLI 조회가 로그인 필요로
실패할 때 [SSH 조회 진단](agy-coding-baseline.md#ssh-환경의-인증-조회)을 확인한다. 인증 저장소 자체가
비어 있거나 로그인 갱신이 필요하면 공식 agy에서 다시 로그인해야 한다.

## 실행·재개

대화형 `xgen`은 기존 승인·TUI·이력·지침 기능을 사용한다. Headless 예시는 다음과 같다.

```bash
./target/debug/xgen run 'README.md를 읽고 요약해' --profile agy --workspace . \
  --allow-dir . --allow-read --allow-remote-model-egress
```

쓰기·process 실행에는 기존 승인이 필요하다. 재개할 때는 원래 workspace·도구 catalog·실행 파일 등록과
필요한 승인을 동일하게 제공한다. agy binary가 업데이트되면 setup으로 새 hash를 등록한 뒤 새 Run을
시작한다. 이전 미완료 Run은 기존 binary/profile 없이는 재개할 수 없다. 완료 Run의 offline replay는 가능하다.
`model use`로 API 프로필로 돌아갈 수 있다. `model logout`은 agy의 공유 로그인을 삭제하지 않는다.

## 자동 업데이트

agy는 마지막 업데이트 확인 뒤 일정 시간(정확한 값은 모름, 18분 만에 다시 확인한 기록이 있다)이 지난 뒤 처음 실행될 때 백그라운드 업데이트 process를 띄워 실행 중인
파일을 새 버전으로 바꾼다. xgen이 실행하는 파일은 private 임시 폴더의 고정 사본이라, 사본이 바뀌면 다음 고정값
검사가 실패해 그 transport의 이후 호출이 모두 `unavailable`이 된다. 업데이트 확인 시각이 새로 기록되므로 바로
다시 실행하면 성공한다. 2026-11-28 원래 agy 실행에서 사본 hash가 바뀌는 것을 2/2 재현했고, xgen은 agy 실행
환경에 `AGY_CLI_DISABLE_AUTO_UPDATE=true`를 넣는다(값 `1`은 끄지 못했다). 업데이트 확인이 필요한 상태로 맞춘 뒤
`tools enrich`를 돌리면 main 빌드는 6/6 호출 실패, 수정 빌드는 공개 명세 3개에서 72/72·69/69·51/51 성공했고 agy
로그에 업데이트 시작이 없었다. 사용자 설치본(`~/.local/bin/agy` 등)의 업데이트는 직접 하고, 바뀐 hash는 setup으로
다시 등록한다. 고정값이 바뀌면 stderr에 `XGEN_AGY code=pin_changed`를 남긴다. 이 변수는 모델 요청이나 agy가 받는
입력을 바꾸지 않으므로 환경 계약 revision(`allowlisted-environment/v1`)과 기존 profile digest는 유지한다.

## 실험 범위

- agy가 제안하고 xgen이 host 도구를 실행한다. agy의 내부 runtime과 자체 기록은 남는다.
- Native tool/subagent 활동은 관찰 후 중단한다. 실행 전 완전 차단은 보장하지 않는다.
- 시간·prompt/stdout/stderr bytes·Run의 논리적 호출 예산을 제한한다. Native 출력 token/USD cap은
  지원하지 않으므로 agy setup에서 `--max-output-tokens`를 받지 않는다.
- 사용량은 cache를 포함한 host 입력으로 변환한다. 누락 값과 실제 비용·내부 요청 수는 UNKNOWN이다.
- agy 인증값·private rules/skills/MCP를 xgen으로 복사하지 않는다. 광범위한 customization 상속은 끈다.
- 새 Rust transport의 실제 모델 smoke는 2026-11-28 headless 7/7 통과([ADR-0090](../adr/0090-experimental-agy-provider.md)).
  기존 Python 경로의 19/20과는 별도 결과다. 대화형 경로는 실제 모델로 검증하지 않았다.
  macOS/Windows의 agy setup은 아직 지원하지 않는다.
