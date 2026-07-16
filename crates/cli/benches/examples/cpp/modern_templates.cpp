#include <array>
#include <concepts>
#include <cstddef>
#include <functional>
#include <optional>
#include <string>
#include <tuple>
#include <type_traits>
#include <utility>
#include <variant>
#include <vector>

namespace benchmark {

template <typename T>
concept Arithmetic = std::is_arithmetic_v<T>;

template <typename T>
concept StringLike = requires(const T &value) {
  { value.size() } -> std::convertible_to<std::size_t>;
  { value.data() };
};

template <Arithmetic T, std::size_t N>
class Vector {
 public:
  constexpr Vector() = default;
  constexpr explicit Vector(std::array<T, N> values) : values_(values) {}

  [[nodiscard]] constexpr T &operator[](std::size_t index) { return values_[index]; }
  [[nodiscard]] constexpr const T &operator[](std::size_t index) const {
    return values_[index];
  }

  template <Arithmetic U>
  [[nodiscard]] constexpr auto dot(const Vector<U, N> &other) const {
    using Result = std::common_type_t<T, U>;
    Result total{};
    for (std::size_t i = 0; i < N; ++i) {
      total += static_cast<Result>(values_[i]) * static_cast<Result>(other[i]);
    }
    return total;
  }

  [[nodiscard]] constexpr auto begin() noexcept { return values_.begin(); }
  [[nodiscard]] constexpr auto end() noexcept { return values_.end(); }
  [[nodiscard]] constexpr auto begin() const noexcept { return values_.begin(); }
  [[nodiscard]] constexpr auto end() const noexcept { return values_.end(); }

 private:
  std::array<T, N> values_{};
};

using Value = std::variant<std::monostate, long, double, std::string>;

struct ValueRenderer {
  std::string operator()(std::monostate) const { return "null"; }
  std::string operator()(long integer) const { return std::to_string(integer); }
  std::string operator()(double number) const { return std::to_string(number); }
  std::string operator()(const std::string &text) const { return '"' + text + '"'; }
};

[[nodiscard]] std::string render(const Value &value) {
  return std::visit(ValueRenderer{}, value);
}

template <StringLike T>
[[nodiscard]] std::optional<std::string> normalize(const T &input) {
  if (input.size() == 0) {
    return std::nullopt;
  }
  std::string output{input.data(), input.size()};
  for (char &character : output) {
    if (character >= 'A' && character <= 'Z') {
      character = static_cast<char>(character - 'A' + 'a');
    }
  }
  return output;
}

template <typename Iterator, typename Projection = std::identity>
requires std::invocable<Projection, typename Iterator::value_type>
auto group_adjacent(Iterator begin, Iterator end,
                    Projection projection = Projection{}) {
  using Source = typename Iterator::value_type;
  using Key = std::invoke_result_t<Projection, Source>;
  std::vector<std::pair<Key, std::vector<Source>>> groups;
  for (auto iterator = begin; iterator != end; ++iterator) {
    Key key = std::invoke(projection, *iterator);
    if (groups.empty() || groups.back().first != key) {
      groups.emplace_back(key, std::vector<Source>{});
    }
    groups.back().second.push_back(*iterator);
  }
  return groups;
}

constexpr Vector<int, 4> left{{1, 2, 3, 4}};
constexpr Vector<double, 4> right{{0.5, 1.5, 2.5, 3.5}};
static_assert(left.dot(right) == 25.0);

}  // namespace benchmark
