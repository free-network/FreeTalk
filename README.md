# FreeTalk

A Forum for freenet

# Building

```sh
# Once
cargo build --package freetalk-delegate --release --target wasm32-unknown-unknown
# Always
npm run watch:css
dx serve --package freetalk-ui
```

# Deploy

Install deploy-tool from [ » Freenet Shared ](https://github.com/free-network/freenet-shared)

Run `deploy-tool deploy`


