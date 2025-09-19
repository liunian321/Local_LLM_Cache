# LLM Local Cache

基于 Rust 和 Axum 框架构建的 LLM API 缓存服务，用于处理聊天请求、模型获取和嵌入生成。通过 SQLite 数据库进行缓存管理，支持与上游 API 进行 HTTP 交互。

## 快速开始

### 核心特性
- 自动缓存API响应，提升响应速度
- 上下文裁切功能，管理聊天上下文长度，防止token超限
- 支持多个上游API端点，根据权重进行请求分发
- 双线程池设计，独立处理缓存命中和未命中

### 快速配置
```yaml
# config.yaml
context_trim:
  enabled: true               # 启用上下文裁切
  max_context_tokens: 4096    # 设置最大token数

api_endpoints:
  - url: "http://127.0.0.1:1234"
    weight: 10
    model: "your-model"
```

启动服务：`cargo run --release`

## 功能描述

1. 聊天请求处理：支持处理聊天请求，并将结果缓存到 SQLite 数据库中
2. 模型获取：提供获取可用模型列表的接口
3. 嵌入生成：支持生成文本嵌入，并返回嵌入结果
4. 缓存管理：使用 SQLite 数据库缓存 API 响应，支持自动检查点和 WAL 模式
5. 备选请求方式：支持使用 `curl` 作为备选请求方式
6. 缓存版本控制：通过环境变量支持缓存版本控制和逐步更新
7. 负载均衡：支持配置多个上游API端点并根据权重进行请求分发
8. 代理支持：可配置使用系统代理访问API
9. 缓存自动维护：支持自动清理过期缓存和维护性能
10. 请求并发控制：支持设置最大并发请求数
11. 统计和监控：提供缓存使用统计信息
12. 双线程池系统：独立的缓存命中和缓存未命中线程池
13. 思考模式支持：可配置是否启用模型的思考功能
14. 上下文裁切功能：裁切聊天上下文，防止超出模型的最大token限制
15. 空闲刷新机制：支持在空闲时批量刷新内存缓存到数据库

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
- `ENABLE_THINKING`: 是否启用思考功能，可设置为 `true`、`false` 或不设置

### 配置文件

项目使用 `config.yaml` 进行配置，包含以下主要设置：

```yaml
database_url: "cache.db"
cache_version: 0
cache_override_mode: true
use_curl: false
use_proxy: true
enable_thinking: false # 是否启用思考功能，可设置为true、false或null
cache_hit_pool_size: 8
cache_miss_pool_size: 8
max_concurrent_requests: 100
# 缓存配置
cache:
  enabled: true               # 是否启用缓存功能
  max_items: 100              # 内存缓存最大条目数量
  batch_write_size: 20        # 批量写入数据库的数量
# 空闲刷新配置
idle_flush:
  enabled: true               # 是否启用空闲刷新功能
  idle_timeout_seconds: 300   # 空闲超时时间（秒）
  check_interval_seconds: 10  # 检查间隔时间（秒）
# 缓存清理配置
cache_maintenance:
  enabled: true                # 是否启用缓存维护
  interval_hours: 12           # 清理间隔时间（小时）
  retention_days: 30           # 保留天数
  cleanup_on_startup: true     # 启动时是否执行清理
  min_hit_count: 1             # 最小命中次数（低于此值的无引用答案会被清理）
# 上下文裁切配置
context_trim:
  enabled: false               # 是否启用上下文裁切功能
  max_context_tokens: 4096     # 允许的最大上下文token数量，超过后会裁切
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
   
   **安装 Rust 和 Cargo：**
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```
   
   **安装 Protocol Buffers 编译器 (protoc)：**
   
   **Windows:**
   ```bash
   # 使用 Winget
   winget install Google.Protobuf
   ```
   
   **macOS:**
   ```bash
   # 使用 Homebrew
   brew install protobuf
   ```
   
   **Linux (Ubuntu/Debian):**
   ```bash
   sudo apt update
   sudo apt install protobuf-compiler
   ```
   
   **Linux (CentOS/RHEL):**
   ```bash
   sudo yum install protobuf-compiler
   ```
   
   **验证安装：**
   ```bash
   protoc --version
   ```
   
   **编译项目：**
   ```bash
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
      "stream": false,
      "enable_thinking": false
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
4. **统计信息**：定期打印缓存统计信息，包括复用率、命中率和内存使用情况。
5. **上下文管理**：通过上下文裁切功能，智能管理聊天上下文长度，防止token超限。
6. **内存优化**：支持内存缓存和数据库缓存的智能切换，提升响应速度。

## 项目结构

- `src/main.rs`: 主程序入口，包含服务器启动逻辑和初始化流程。
- `src/server.rs`: 服务器及路由配置，负责API路由分发和请求处理。
- `src/lib.rs`: 包含项目模块导出。
- `src/handlers/`: 请求处理模块，包含聊天、模型获取和嵌入生成的处理逻辑。
- `src/models/`: 数据模型定义。
- `src/proto/`: Protocol Buffers 定义文件，包含API接口的数据结构定义。
- `src/utils/`: 工具函数集合，包括：
  - `config.rs`: 配置加载和处理
  - `db.rs`: 数据库操作和管理
  - `http_client.rs`: HTTP客户端创建
  - `cache_maintenance.rs`: 缓存维护和统计功能
  - `context_trim.rs`: 上下文裁切功能，智能管理聊天上下文长度
  - `idle_flush.rs`: 空闲刷新机制，批量刷新内存缓存到数据库
  - `memory_cache.rs`: 内存缓存管理

### 参数说明

- **enable_thinking**：控制是否启用模型的思考功能。
  - 当设置为 `true` 时：模型会先思考再回答，通常生成更深入的回复。
  - 当设置为 `false` 时：模型会直接回答，不进行额外思考过程。
  - 当设置为 `null` 或不设置时：不向上游API传递此参数，使用上游API的默认行为。

- **context_trim**：上下文裁切功能配置。
  - `enabled`：是否启用上下文裁切功能，默认为 `false`。
  - `max_context_tokens`：最大上下文token数量，建议设置为模型最大token数的70-80%，默认为 `4096`。

- **idle_flush**：空闲刷新机制配置。
  - `enabled`：是否启用空闲刷新功能，默认为 `false`。
  - `idle_timeout_seconds`：空闲超时时间（秒），默认为 `300`。
  - `check_interval_seconds`：检查间隔时间（秒），默认为 `10`。

- **cache**：内存缓存配置。
  - `enabled`：是否启用缓存功能，默认为 `true`。
  - `max_items`：内存缓存最大条目数量，默认为 `100`。
  - `batch_write_size`：批量写入数据库的数量，默认为 `20`。

---

# LLM API Cache Service

LLM API cache service built with Rust and Axum framework for handling chat requests, model retrieval, and embedding generation. Uses SQLite for cache management and supports HTTP interaction with upstream APIs.

## Quick Start

### Core Features
- Automatically caches API responses to improve response speed
- Context trimming functionality to manage chat context length and prevent token overflow
- Supports multiple upstream API endpoints with weight-based request distribution
- Dual thread pool design for independent cache hit and miss processing

### Quick Configuration
```yaml
# config.yaml
context_trim:
  enabled: true               # Enable context trimming
  max_context_tokens: 4096    # Maximum context token count, exceeding will be trimmed

api_endpoints:
  - url: "http://127.0.0.1:1234"
    weight: 10
    model: "your-model"
```

Start service: `cargo run --release`

## Features

1. Chat Request Handling: Processes chat requests and caches the results in a SQLite database
2. Model Retrieval: Provides an interface to retrieve a list of available models
3. Embedding Generation: Supports generating text embeddings and returns the embedding results
4. Cache Management: Uses SQLite to cache API responses, supports automatic checkpoints and WAL mode
5. Alternative Request Method: Supports using `curl` as an alternative request method
6. Cache Version Control: Supports cache version control and gradual cache updates using environment variables
7. Load Balancing: Supports configuring multiple upstream API endpoints and request distribution based on weight
8. Proxy Support: Can configure using system proxy to access API
9. Cache Auto Maintenance: Supports automatic cache cleanup and maintenance for optimal performance
10. Request Concurrency Control: Supports setting maximum concurrent request count
11. Statistics and Monitoring: Provides cache usage statistics
12. Dual Thread Pool System: Separate cache hit and cache miss thread pools
13. Thinking Mode Support: Can configure whether to enable thinking functionality for models
14. Context Trimming Functionality: Trims chat context to prevent exceeding the model's maximum token limit
15. Idle Flush Mechanism: Supports batch flushing memory cache to database during idle periods
16. Protocol Buffers Support: Uses protobuf for data serialization and deserialization

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
- `ENABLE_THINKING`: Whether to enable thinking functionality, can be set to `true`, `false`, or not set

### Configuration File

The project uses `config.yaml` for configuration, which includes the following main settings:

```yaml
database_url: "cache.db"
cache_version: 0
cache_override_mode: true
use_curl: false
use_proxy: true
enable_thinking: false # Whether to enable thinking functionality, can be set to true, false, or null
cache_hit_pool_size: 8
cache_miss_pool_size: 8
max_concurrent_requests: 100
# Cache configuration
cache:
  enabled: true               # Whether to enable cache functionality
  max_items: 100              # Maximum number of memory cache entries
  batch_write_size: 20        # Batch write size to database
# Idle flush configuration
idle_flush:
  enabled: true               # Whether to enable idle flush functionality
  idle_timeout_seconds: 300   # Idle timeout time (seconds)
  check_interval_seconds: 10  # Check interval time (seconds)
# Cache cleanup configuration
cache_maintenance:
  enabled: true                # Whether to enable cache maintenance
  interval_hours: 12           # Cleanup interval time (hours)
  retention_days: 30           # Retention days
  cleanup_on_startup: true     # Whether to perform cleanup on startup
  min_hit_count: 1             # Minimum hit count (answers below this value will be cleaned up)
# Context trimming configuration
context_trim:
  enabled: false               # Whether to enable context trimming functionality
  max_context_tokens: 4096     # Maximum context token count, recommended to be 70-80% of model max token count
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

#### Configuration Options

- `enabled`: Whether to enable context trimming functionality
- `max_context_tokens`: Maximum context token count, recommended to be 70-80% of the model's maximum token count

### Starting the Service

1. Clone the project locally:
   ```bash
   git clone https://github.com/liunian321/Local_LLM_Cache.git
   cd Local_LLM_Cache
   ```

2. Install dependencies and compile the project:
   
   **Install Rust and Cargo:**
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```
   
   **Install Protocol Buffers compiler (protoc):**
   
   **Windows:**
   ```bash
   # Using Winget
   winget install Google.Protobuf
   ```
   
   **macOS:**
   ```bash
   # Using Homebrew
   brew install protobuf
   ```
   
   **Linux (Ubuntu/Debian):**
   ```bash
   sudo apt update
   sudo apt install protobuf-compiler
   ```
   
   **Linux (CentOS/RHEL):**
   ```bash
   sudo yum install protobuf-compiler
   ```
   
   **Verify installation:**
   ```bash
   protoc --version
   ```
   
   **Compile the project:**
   ```bash
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
      "stream": false,
      "enable_thinking": false
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
    messages=[{"role": "user", "content": "Hello, how are you?"}]
)
print(response.choices[0].message.content)
```

## Cache Maintenance

The service supports automatic cache maintenance functionality, which can be implemented through configuration:

1. **Periodic Cleanup**: Can set cleanup interval time to automatically clean up expired cache entries.
2. **Retention Strategy**: Can set retention days and minimum hit count to optimize storage space utilization.
3. **Startup Cleanup**: Can choose to perform cleanup on startup to ensure optimal service performance.
4. **Statistics Information**: Periodically print cache statistics information, including hit rate, total size, and hot entries.
5. **Context Management**: Through context trimming functionality, intelligently manages chat context length to prevent token overflow.
6. **Memory Optimization**: Supports intelligent switching between memory cache and database cache to improve response speed.

## Project Structure

- `src/main.rs`: Main program entry, contains server startup logic and initialization process.
- `src/server.rs`: Server and route configuration, responsible for API route distribution and request handling.
- `src/lib.rs`: Includes project module exports.
- `src/handlers/`: Request processing module, including chat, model retrieval, and embedding generation processing logic.
- `src/models/`: Data model definition.
- `src/proto/`: Protocol Buffers definition files, containing API interface data structure definitions.
- `src/utils/`: Tool function collection, including:
  - `config.rs`: Configuration loading and processing
  - `db.rs`: Database operation and management
  - `http_client.rs`: HTTP client creation
  - `cache_maintenance.rs`: Cache maintenance and statistics functionality
  - `context_trim.rs`: Context trimming functionality, intelligently manages chat context length
  - `idle_flush.rs`: Idle flush mechanism, batch flushes memory cache to database
  - `memory_cache.rs`: Memory cache management

### Parameter Description

- **enable_thinking**: Controls whether to enable the thinking functionality of the model.
  - When set to `true`: The model will think before answering, typically generating more in-depth replies.
  - When set to `false`: The model will answer directly, without additional thinking process.
  - When set to `null` or not set: This parameter will not be passed to the upstream API, using the default behavior of the upstream API.

- **context_trim**: Context trimming functionality configuration.
  - `enabled`: Whether to enable context trimming functionality, defaults to `false`.
  - `max_context_tokens`: Maximum context token count, recommended to be 70-80% of the model's maximum token count, defaults to `4096`.

- **idle_flush**: Idle flush mechanism configuration.
  - `enabled`: Whether to enable idle flush functionality, defaults to `false`.
  - `idle_timeout_seconds`: Idle timeout time (seconds), defaults to `300`.
  - `check_interval_seconds`: Check interval time (seconds), defaults to `10`.

- **cache**: Memory cache configuration.
  - `enabled`: Whether to enable cache functionality, defaults to `true`.
  - `max_items`: Maximum number of memory cache entries, defaults to `100`.
  - `batch_write_size`: Batch write size to database, defaults to `20`.
