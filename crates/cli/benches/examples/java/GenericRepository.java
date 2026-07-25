package example.repository;

import java.lang.annotation.ElementType;
import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;
import java.lang.annotation.Target;
import java.util.ArrayList;
import java.util.Collection;
import java.util.Comparator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.function.Predicate;

@Target({ElementType.TYPE, ElementType.METHOD})
@Retention(RetentionPolicy.RUNTIME)
@interface Transactional {
    boolean readOnly() default false;
}

public final class GenericRepository<K, V extends GenericRepository.Entity<K>> {
    public interface Entity<K> {
        K id();
        long version();
    }

    public record Page<T>(List<T> values, int offset, int total) {
        public Page {
            values = List.copyOf(values);
            if (offset < 0 || total < values.size()) {
                throw new IllegalArgumentException("invalid page");
            }
        }
    }

    private final Map<K, V> values = new LinkedHashMap<>();

    @Transactional
    public V save(V value) {
        Objects.requireNonNull(value, "value");
        V previous = values.get(value.id());
        if (previous != null && previous.version() >= value.version()) {
            throw new IllegalStateException("stale entity " + value.id());
        }
        values.put(value.id(), value);
        return value;
    }

    @Transactional(readOnly = true)
    public Optional<V> find(K key) {
        return Optional.ofNullable(values.get(key));
    }

    @Transactional(readOnly = true)
    public Page<V> search(Predicate<? super V> predicate, Comparator<? super V> order,
                          int offset, int limit) {
        if (offset < 0 || limit < 1) {
            throw new IllegalArgumentException("invalid range");
        }
        List<V> matches = values.values().stream()
                .filter(predicate)
                .sorted(order)
                .toList();
        int start = Math.min(offset, matches.size());
        int end = Math.min(start + limit, matches.size());
        return new Page<>(matches.subList(start, end), start, matches.size());
    }

    @Transactional
    public List<V> saveAll(Collection<? extends V> additions) {
        List<V> saved = new ArrayList<>(additions.size());
        for (V value : additions) {
            saved.add(save(value));
        }
        return List.copyOf(saved);
    }

    @Transactional
    public boolean delete(K key, long expectedVersion) {
        return values.computeIfPresent(key, (ignored, current) -> {
            if (current.version() != expectedVersion) {
                throw new IllegalStateException("version mismatch");
            }
            return null;
        }) == null;
    }
}
