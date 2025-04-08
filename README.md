# LLM Local Cache

这是一个基于 Rust 和 Axum 框架构建的轻量级高并发 LLM (Large Language Model) API 缓存服务，用于处理聊天请求、模型获取和嵌入生成。项目通过 SQLite 数据库进行缓存管理，通过 HTTP 请求与上游 API 进行交互。

## 功能描述

1. **聊天请求处理**：支持处理聊天请求，并将结果缓存到 SQLite 数据库中，以提高后续相同请求的响应速度。
2. **模型获取**：提供获取可用模型列表的接口。
3. **嵌入生成**：支持生成文本嵌入，并返回嵌入结果。
4. **缓存管理**：使用 SQLite 数据库缓存 API 响应，支持自动检查点和 WAL 模式，确保数据一致性和性能。
5. **备选请求方式**：支持使用 `curl` 作为备选请求方式，确保在主请求方式失败时仍能正常处理请求。
6. **缓存版本控制**：通过 `CACHE_VERSION` 和 `CACHE_OVERRIDE_MODE` 环境变量，支持缓存版本控制和逐步更新已有缓存。

## 如何使用

### 环境变量配置

解释

- `DATABASE_URL`: SQLite 数据库的路径，默认为 `cache.db`。
- `API_URL`: 上游 API 的地址，默认为 `http://127.0.0.1:1234`。
- `USE_CURL`: 是否使用 `curl` 作为备选请求方式，默认为 `false`。
- `CACHE_VERSION`: 用于搭配 `CACHE_OVERRIDE_MODE` 决定是否覆盖旧缓存。
- `CACHE_OVERRIDE_MODE`: 是否开启缓存覆盖模式（如果你需要逐步更新已有的缓存的话）。
- `CACHE_MISS_POOL_SIZE`: 缓存未命中线程池大小，默认为 `4`。
- `CACHE_HIT_POOL_SIZE`: 缓存命中线程池大小，默认为 `64`。

### 启动服务

1. 克隆项目到本地：
   ```bash
   git clone https://github.com/liunian321/Local_LLM_Cache.git
   cd Local_LLM_Cache
   ```

2. 安装依赖并编译项目：
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   cargo build --release
   ```

3. 运行服务：
   ```bash
   cargo run --release
   ```

### API 接口

- **聊天请求**：
  - 路径：`/v1/chat/completions`
  - 方法：`POST`
  - 请求体：
    ```json
    {
      "model": "gpt-3.5-turbo",
      "messages": [
        {
          "role": "user",
          "content": "Hello, how are you?"
        }
      ],
      "temperature": 0.1,
      "max_tokens": -1,
      "stream": false
    }
    ```

- **获取模型列表**：
  - 路径：`/v1/models`
  - 方法：`GET`

- **生成嵌入**：
  - 路径：`/v1/embeddings`
  - 方法：`POST`
  - 请求体：
    ```json
    {
      "input": "This is a sample text for embedding."
    }
    ```

## 项目结构

- `src/main.rs`: 主程序入口，包含路由定义和请求处理逻辑。
- `src/lib.rs`: 包含 proto 模块的引入。
- `src/proto/api.proto`: 定义 API 的 proto 文件。
- `src/handlers/api_handler.rs`: 处理 API 请求的模块，包含聊天请求、模型获取和嵌入生成的逻辑。
- `src/models/api_model.rs`: 定义 API 请求和响应的数据结构。

---

# LLM API Cache Service

This is a lightweight, high-concurrency LLM (Large Language Model) API cache service built with Rust and the Axum framework, designed to handle chat requests, model retrieval, and embedding generation. The project uses SQLite for cache management and supports interaction with an upstream API via HTTP requests.

## Features

1. **Chat Request Handling**: Processes chat requests and caches the results in a SQLite database to improve response times for subsequent identical requests.
2. **Model Retrieval**: Provides an interface to retrieve a list of available models.
3. **Embedding Generation**: Supports generating text embeddings and returns the embedding results.
4. **Cache Management**: Uses SQLite to cache API responses, supports automatic checkpoints and WAL mode to ensure data consistency and performance.
5. **Alternative Request Method**: Supports using `curl` as an alternative request method to ensure requests can still be processed if the primary method fails.
6. **Cache Version Control**: Supports cache version control and gradual cache updates using `CACHE_VERSION` and `CACHE_OVERRIDE_MODE` environment variables.

## How to Use

### Environment Variables Configuration

- `DATABASE_URL`: Path to the SQLite database, defaults to `cache.db`.
- `API_URL`: Address of the upstream API, defaults to `http://127.0.0.1:1234`.
- `USE_CURL`: Whether to use `curl` as an alternative request method, defaults to `false`.
- `CACHE_VERSION`: Used in conjunction with `CACHE_OVERRIDE_MODE` to determine whether to override old cache.
- `CACHE_OVERRIDE_MODE`: Whether to enable cache override mode (if you need to gradually update the existing cache).
- `CACHE_MISS_POOL_SIZE`: Size of the cache miss thread pool, defaults to `4`.
- `CACHE_HIT_POOL_SIZE`: Size of the cache hit thread pool, defaults to `64`.

### Starting the Service

1. Clone the project locally:
   ```bash
   git clone https://github.com/liunian321/Local_LLM_Cache.git
   cd Local_LLM_Cache
   ```

2. Install dependencies and compile the project:
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   cargo build --release
   ```

3. Run the service:
   ```bash
   cargo run --release
   ```

### API Endpoints

- **Chat Request**:
  - Path: `/v1/chat/completions`
  - Method: `POST`
  - Request Body:
    ```json
    {
      "model": "gpt-3.5-turbo",
      "messages": [
        {
          "role": "user",
          "content": "Hello, how are you?"
        }
      ],
      "temperature": 0.1,
      "max_tokens": -1,
      "stream": false
    }
    ```

- **Retrieve Model List**:
  - Path: `/v1/models`
  - Method: `GET`

- **Generate Embedding**:
  - Path: `/v1/embeddings`
  - Method: `POST`
  - Request Body:
    ```json
    {
      "input": "This is a sample text for embedding."
    }
    ```

## Project Structure

- `src/main.rs`: Main program entry, contains route definitions and request handling logic.
- `src/lib.rs`: Includes the proto module.
- `src/proto/api.proto`: Defines the API proto file.
- `src/handlers/api_handler.rs`: Module handling API requests, including chat requests, model retrieval, and embedding generation logic.
- `src/models/api_model.rs`: Defines data structures for API requests and responses.
