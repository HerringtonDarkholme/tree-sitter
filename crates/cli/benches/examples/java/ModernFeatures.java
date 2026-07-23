package example.modern;

import java.time.Instant;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.Function;

public final class ModernFeatures {
    public sealed interface Event permits Created, Updated, Deleted {
        String key();
        Instant timestamp();
    }

    public record Created(String key, Instant timestamp, Map<String, String> attributes)
            implements Event {
        public Created {
            Objects.requireNonNull(key);
            attributes = Map.copyOf(attributes);
        }
    }

    public record Updated(String key, Instant timestamp, String field, String value)
            implements Event {}

    public record Deleted(String key, Instant timestamp, Optional<String> reason)
            implements Event {}

    private final Map<String, List<Event>> events = new ConcurrentHashMap<>();

    public void append(Event event) {
        events.computeIfAbsent(event.key(), ignored -> new ArrayList<>()).add(event);
    }

    public String describe(Event event) {
        return switch (event) {
            case Created created -> "created:" + created.attributes().size();
            case Updated updated -> "updated:" + updated.field() + "=" + updated.value();
            case Deleted deleted -> "deleted:" + deleted.reason().orElse("unknown");
        };
    }

    public <T> List<T> map(String key, Function<? super Event, ? extends T> mapper) {
        List<T> result = new ArrayList<>();
        for (Event event : events.getOrDefault(key, List.of())) {
            result.add(mapper.apply(event));
        }
        return List.copyOf(result);
    }

    public Optional<Event> latest(String key) {
        List<Event> values = events.get(key);
        if (values == null || values.isEmpty()) {
            return Optional.empty();
        }
        return Optional.of(values.get(values.size() - 1));
    }

    public static ModernFeatures sample() {
        ModernFeatures result = new ModernFeatures();
        Instant now = Instant.now();
        result.append(new Created("parser", now, Map.of("language", "java")));
        result.append(new Updated("parser", now.plusSeconds(1), "state", "ready"));
        result.append(new Deleted("parser", now.plusSeconds(2), Optional.of("complete")));
        return result;
    }
}
