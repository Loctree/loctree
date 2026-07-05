FROM node:20-alpine
ENV NODE_ENV=production API_PORT=3000
ENV LOG_LEVEL=info
WORKDIR /app
COPY package.json ./
RUN npm install --omit=dev
COPY . .
CMD ["node", "server.js"]
