#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>
#include <string_view>
#include <type_traits>

#if defined(_WIN32)
constexpr std::uint32_t platform_tag = 1;
#elif defined(__APPLE__)
constexpr std::uint32_t platform_tag = 2;
#else
constexpr std::uint32_t platform_tag = 3;
#endif

namespace benchmark {
namespace syntax {

enum class Kind : std::uint8_t {
  unknown = 0,
  identifier = 1,
  number = 2,
  string = 3,
  comment = 4,
};

struct Point {
  std::uint32_t row;
  std::uint32_t column;

  friend constexpr bool operator==(Point, Point) = default;
};

struct Range {
  Point start;
  Point end;
  std::uint32_t start_byte;
  std::uint32_t end_byte;

  [[nodiscard]] constexpr bool empty() const noexcept {
    return start_byte == end_byte;
  }
};

template <typename T>
class Handle final {
 public:
  using element_type = T;

  constexpr Handle() noexcept = default;
  explicit constexpr Handle(T *pointer) noexcept : pointer_(pointer) {}

  [[nodiscard]] constexpr T *get() const noexcept { return pointer_; }
  [[nodiscard]] constexpr T &operator*() const noexcept { return *pointer_; }
  [[nodiscard]] constexpr T *operator->() const noexcept { return pointer_; }
  [[nodiscard]] explicit constexpr operator bool() const noexcept {
    return pointer_ != nullptr;
  }

 private:
  T *pointer_ = nullptr;
};

class Document {
 public:
  struct Options {
    bool include_comments;
    bool recover_errors;
    std::uint16_t maximum_depth;
  };

  explicit Document(std::string_view source, Options options);
  Document(const Document &) = delete;
  Document &operator=(const Document &) = delete;
  Document(Document &&) noexcept;
  Document &operator=(Document &&) noexcept;
  ~Document();

  [[nodiscard]] std::string_view source() const noexcept;
  [[nodiscard]] std::size_t node_count() const noexcept;
  [[nodiscard]] Range changed_range(const Document &previous) const;

 private:
  class Implementation;
  std::unique_ptr<Implementation> implementation_;
};

extern "C" {
Document *benchmark_document_new(const char *source, std::size_t length);
void benchmark_document_delete(Document *document);
std::size_t benchmark_document_node_count(const Document *document);
}

template <typename T>
inline constexpr bool is_handle_v = false;

template <typename T>
inline constexpr bool is_handle_v<Handle<T>> = true;

static_assert(std::is_trivially_copyable_v<Point>);
static_assert(std::is_standard_layout_v<Range>);

}  // namespace syntax
}  // namespace benchmark
