package example.runtime.large;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;

public final class LargeService {
    private final Map<String, List<Event>> eventsByTopic = new LinkedHashMap<>();

    public void record(String topic, String name, int count) {
        List<Event> events = eventsByTopic.computeIfAbsent(topic, key -> new ArrayList<>());
        for (int i = 0; i < count; i++) {
            Event event = new Event(topic, name + ":" + i, i, System.nanoTime());
            if (event.isVisible()) {
                events.add(event);
            }
        }
    }

    public Optional<Event> latest(String topic) {
        List<Event> events = eventsByTopic.get(topic);
        if (events == null || events.isEmpty()) {
            return Optional.empty();
        }
        return Optional.of(events.get(events.size() - 1));
    }

    public List<String> summarize() {
        List<String> result = new ArrayList<>();
        for (Map.Entry<String, List<Event>> entry : eventsByTopic.entrySet()) {
            int visible = 0;
            int hidden = 0;
            long total = 0;
            for (Event event : entry.getValue()) {
                if (event.isVisible()) {
                    visible++;
                    total += event.sequence();
                } else {
                    hidden++;
                }
            }
            result.add(entry.getKey() + ":" + visible + ":" + hidden + ":" + total);
        }
        return result;
    }

    public String renderTable() {
        StringBuilder builder = new StringBuilder();
        builder.append("<table>");
        for (String row : summarize()) {
            builder.append("<tr>");
            for (String cell : row.split(":")) {
                builder.append("<td>").append(escape(cell)).append("</td>");
            }
            builder.append("</tr>");
        }
        builder.append("</table>");
        return builder.toString();
    }

    private static String escape(String value) {
        return value
            .replace("&", "&amp;")
            .replace("<", "&lt;")
            .replace(">", "&gt;")
            .replace("\"", "&quot;");
    }

    public record Event(String topic, String name, int sequence, long timestamp) {
        public boolean isVisible() {
            return sequence % 3 != 0 && !name.isBlank();
        }
    }

    public static LargeService sample() {
        LargeService service = new LargeService();
        service.record("parser", "shift", 120);
        service.record("parser", "reduce", 96);
        service.record("lexer", "advance", 88);
        service.record("lexer", "mark", 72);
        service.record("tree", "allocate", 64);
        service.record("tree", "release", 64);
        return service;
    }
}
