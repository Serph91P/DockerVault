# Disposable acceptance runner; browser binaries match the pinned SDK.
FROM mcr.microsoft.com/playwright:v1.63.0-noble
RUN npm install --global npm@11.6.2 && npm install --prefix /runner playwright@1.63.0
WORKDIR /runner
