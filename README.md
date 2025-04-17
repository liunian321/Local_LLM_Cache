# LLM Local Cache

这是一个基于 Rust 和 Axum 框架构建的轻量级高并发 LLM (Large Language Model) API 缓存服务，用于处理聊天请求、模型获取和嵌入生成。项目通过 SQLite 数据库进行缓存管理，通过 HTTP 请求与上游 API 进行交互。

## 功能描述

1.  **聊天请求处理**：支持处理聊天请求，并将结果缓存到 SQLite 数据库中，以提高后续相同请求的响应速度。
2.  **模型获取**：提供获取可用模型列表的接口。
3.  **嵌入生成**：支持生成文本嵌入，并返回嵌入结果。
4.  **缓存管理**：使用 SQLite 数据库缓存 API 响应，支持自动检查点和 WAL 模式，确保数据一致性和性能。
5.  **备选请求方式**：支持使用 `curl` 作为备选请求方式，确保在主请求方式失败时仍能正常处理请求。
6.  **缓存版本控制**：通过 `CACHE_VERSION` 和 `CACHE_OVERRIDE_MODE` 环境变量，支持缓存版本控制和逐步更新已有缓存。
7.  **负载均衡**：支持配置多个上游API端点并根据权重进行智能请求分发。
8.  **代理支持**：可配置使用系统代理访问API，便于在网络受限环境中使用。
9.  **缓存自动维护**：支持自动清理过期缓存和维护最佳性能，包括定期清理无引用答案和过期问题记录。
10. **请求并发控制**：支持设置最大并发请求数，防止系统过载。
11. **统计和监控**：提供缓存使用统计信息，包括复用率、命中率和内存使用情况。
12. **双线程池系统**：独立的缓存命中和缓存未命中线程池，提升服务性能。

## 如何使用

### 环境变量配置

以下环境变量可用于配置服务：

- `DATABASE_URL`: SQLite 数据库的路径，默认为 `cache.db`
- `USE_CURL`: 是否使用 `curl` 作为备选请求方式，默认为 `false`
- `CACHE_VERSION`: 缓存版本号，用于控制缓存更新
- `CACHE_OVERRIDE_MODE`: 缓存覆盖模式，设置为 `true` 时会覆盖同版本缓存
- `CACHE_MISS_POOL_SIZE`: 缓存未命中线程池大小，默认为 `8`
- `CACHE_HIT_POOL_SIZE`: 缓存命中线程池大小，默认为 `8`
- `MAX_CONCURRENT_REQUESTS`: 最大并发请求数，默认为 `100`
- `USE_PROXY`: 是否使用系统代理，默认为 `true`

### 配置文件

项目使用 `config.yaml` 进行配置，包含以下主要设置：

```yaml
database_url: "cache.db"
cache_version: 0
cache_override_mode: true
use_curl: false
use_proxy: true
cache_hit_pool_size: 8
cache_miss_pool_size: 8
max_concurrent_requests: 100
# 缓存清理配置
cache_maintenance:
  enabled: true                # 是否启用缓存维护
  interval_hours: 12           # 清理间隔时间（小时）
  retention_days: 30           # 保留天数
  cleanup_on_startup: true     # 启动时是否执行清理
  min_hit_count: 1             # 最小命中次数（低于此值的无引用答案会被清理）
api_headers:
  Content-Type: "application/json"
  Accept: "application/json"
  User-Agent: "llm_api_rust_client/1.0"
api_endpoints:
  - url: "http://127.0.0.1:1234"
    weight: 10
    version: 0
    model: "model-name-1"
  - url: "http://127.0.0.1:11434"
    weight: 0
    version: 0
    model: "model-name-2"
```

其中，`api_endpoints` 配置允许设置多个上游 API 端点，每个端点包含：
- `url`: API 端点地址
- `weight`: 权重值，用于负载均衡（权重越高被选中概率越大）
- `version`: 版本号，用于缓存版本控制
- `model`: 模型名称，可以覆盖请求中指定的模型名称

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

3. 配置 `config.yaml` 文件：
   创建或修改项目根目录下的 `config.yaml` 文件，设置你的上游API端点和其他配置。

4. 运行服务：
   ```bash
   cargo run --release
   ```
   
5. 服务默认在 `http://127.0.0.1:4321` 启动，可以在`server.rs`中修改端口。

### API 接口

- **聊天请求**：
  - 路径：`/v1/chat/completions` 或 `/chat/completions`
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
  - 路径：`/v1/models` 或 `/models`
  - 方法：`GET`

- **生成嵌入**：
  - 路径：`/v1/embeddings` 或 `/embeddings`
  - 方法：`POST`
  - 请求体：
    ```json
    {
      "model": "text-embedding-ada-002",
      "input": "This is a sample text for embedding."
    }
    ```

### 客户端配置示例

如果你使用OpenAI客户端，可以设置基础URL指向本服务：

```python
from openai import OpenAI

client = OpenAI(
    base_url="http://127.0.0.1:4321/v1",
    api_key="dummy-key"  # 本服务不验证API密钥
)

response = client.chat.completions.create(
    model="gpt-3.5-turbo",
    messages=[{"role": "user", "content": "你好！"}]
)
print(response.choices[0].message.content)
```

## 缓存维护

服务支持自动缓存维护功能，可以通过配置实现：

1. **定期清理**：可设置清理间隔时间，自动清理过期的缓存条目。
2. **留存策略**：可设置保留天数和最小命中次数，优化存储空间利用。
3. **启动时清理**：可选择在服务启动时执行清理，确保服务始终有最佳性能。
4. **统计信息**：定期打印缓存统计信息，包括复用率、总大小和热点条目。

## 项目结构

- `src/main.rs`: 主程序入口，包含服务器启动逻辑和初始化流程。
- `src/server.rs`: 服务器及路由配置，负责API路由分发和请求处理。
- `src/lib.rs`: 包含项目模块导出。
- `src/handlers/`: 请求处理模块，包含聊天、模型获取和嵌入生成的处理逻辑。
- `src/models/`: 数据模型定义。
- `src/utils/`: 工具函数集合，包括：
  - `config.rs`: 配置加载和处理
  - `db.rs`: 数据库操作和管理
  - `http_client.rs`: HTTP客户端创建
  - `cache_maintenance.rs`: 缓存维护和统计功能

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
7. **Load Balancing**: Supports configuring multiple upstream API endpoints and smart request distribution based on weight.
8. **Proxy Support**: Can configure using system proxy to access API, convenient in network-restricted environments.
9. **Cache Auto Maintenance**: Supports automatic cache cleanup and maintenance for optimal performance, including periodic cleanup of unused answers and expired question records.
10. **Request Concurrency Control**: Supports setting maximum concurrent request count to prevent system overload.
11. **Statistics and Monitoring**: Provides cache usage statistics, including hit rate, hit count, and memory usage.
12. **Dual Thread Pool System**: Separate cache hit and cache miss thread pools for improved service performance.

## How to Use

### Environment Variables Configuration

The following environment variables can be used to configure the service:

- `DATABASE_URL`: Path to the SQLite database, defaults to `cache.db`
- `USE_CURL`: Whether to use `curl` as an alternative request method, defaults to `false`
- `CACHE_VERSION`: Cache version number, used to control cache updates
- `CACHE_OVERRIDE_MODE`: Cache override mode, set to `true` to override same version cache
- `CACHE_MISS_POOL_SIZE`: Size of the cache miss thread pool, defaults to `8`
- `CACHE_HIT_POOL_SIZE`: Size of the cache hit thread pool, defaults to `8`
- `MAX_CONCURRENT_REQUESTS`: Maximum concurrent request count, defaults to `100`
- `USE_PROXY`: Whether to use system proxy, defaults to `true`

### Configuration File

The project uses `config.yaml` for configuration, which includes the following main settings:

```yaml
database_url: "cache.db"
cache_version: 0
cache_override_mode: true
use_curl: false
use_proxy: true
cache_hit_pool_size: 8
cache_miss_pool_size: 8
max_concurrent_requests: 100
# Cache cleanup configuration
cache_maintenance:
  enabled: true                # Whether to enable cache maintenance
  interval_hours: 12           # Cleanup interval time (hours)
  retention_days: 30           # Retention days
  cleanup_on_startup: true     # Whether to perform cleanup on startup
  min_hit_count: 1             # Minimum hit count (answers below this value will be cleaned up)
api_headers:
  Content-Type: "application/json"
  Accept: "application/json"
  User-Agent: "llm_api_rust_client/1.0"
api_endpoints:
  - url: "http://127.0.0.1:1234"
    weight: 10
    version: 0
    model: "model-name-1"
  - url: "http://127.0.0.1:11434"
    weight: 0
    version: 0
    model: "model-name-2"
```

The `api_endpoints` configuration allows setting multiple upstream API endpoints, each containing:
- `url`: API endpoint address
- `weight`: Weight value for load balancing (higher weight means higher probability of being selected)
- `version`: Version number for cache version control
- `model`: Model name, can override the model name specified in the request

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

3. Configure the `config.yaml` file:
   Create or modify the `config.yaml` file in the project root directory to set your upstream API endpoints and other configurations.

4. Run the service:
   ```bash
   cargo run --release
   ```
   
5. The service defaults to starting at `http://127.0.0.1:4321`, which can be changed by modifying the configuration file.

### API Endpoints

- **Chat Request**:
  - Path: `/v1/chat/completions` or `/chat/completions`
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
  - Path: `/v1/models` or `/models`
  - Method: `GET`

- **Generate Embedding**:
  - Path: `/v1/embeddings` or `/embeddings`
  - Method: `POST`
  - Request Body:
    ```json
    {
      "model": "text-embedding-ada-002",
      "input": "This is a sample text for embedding."
    }
    ```

### Client Configuration Example

If you use the OpenAI client, you can set the base URL to point to this service:

```python
from openai import OpenAI

client = OpenAI(
    base_url="http://127.0.0.1:4321/v1",
    api_key="dummy-key"  # This service does not verify API key
)

response = client.chat.completions.create(
    model="gpt-3.5-turbo",
    messages=[{"role": "user", "content": "你好！"}]
)
print(response.choices[0].message.content)
```

## Cache Maintenance

The service supports automatic cache maintenance functionality, which can be implemented through configuration:

1. **Periodic Cleanup**: Can set cleanup interval time to automatically clean up expired cache entries.
2. **Retention Strategy**: Can set retention days and minimum hit count to optimize storage space utilization.
3. **Startup Cleanup**: Can choose to perform cleanup on startup to ensure optimal service performance.
4. **Statistics Information**: Periodically print cache statistics information, including hit rate, total size, and hot entries.

## Project Structure

- `src/main.rs`: Main program entry, contains server startup logic and initialization process.
- `src/server.rs`: Server and route configuration, responsible for API route distribution and request handling.
- `src/lib.rs`: Includes project module exports.
- `src/handlers/`: Request processing module, including chat, model retrieval, and embedding generation processing logic.
- `src/models/`: Data model definition.
- `src/utils/`: Tool function collection, including:
  - `config.rs`: Configuration loading and processing
  - `db.rs`: Database operation and management
  - `http_client.rs`: HTTP client creation
  - `cache_maintenance.rs`: Cache maintenance and statistics functionality
