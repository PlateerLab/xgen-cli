# `@xgen-ai/cli`

터미널에서 프로젝트를 읽고 수정하고 테스트하는 XGEN CLI의 native 실행 파일을 설치해.
현재 Developer Preview야. Node.js 22.14 이상이 필요해.

```bash
npm install --global @xgen-ai/cli
xgen --version
cd my-project
xgen
```

첫 실행에서 provider, 숨김 API key와 model을 선택하면 연결을 검증하고 설정을 저장해.
최초 stable 게시 전까지 기본 설치는 검증된 최신 preview를 가리키고, 그 뒤에는 stable을 따라.
Preview 채널은 `@xgen-ai/cli@next`, 고정 설치는 `@xgen-ai/cli@0.1.0-rc.10`이야.

현재 OS/CPU의 exact-version optional dependency 하나에 담긴 Rust 실행 파일을 사용해.
설치 script, 추가 download, native compile은 없어. `--omit=optional`은 실행 파일을 빼므로 지원하지 않아.
Node.js 없이 설치하려면 같은 version의 GitHub Release를 사용해.

모델 연결·권한·복구는 [시작하기](https://github.com/PlateerLab/xgen-cli/blob/main/docs/getting-started.md)를 참고해.
