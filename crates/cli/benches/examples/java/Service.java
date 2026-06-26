package example.runtime;

import java.util.ArrayList;
import java.util.List;

public final class Service {
    private final List<String> events = new ArrayList<>();

    public void record(String name, int count) {
        for (int i = 0; i < count; i++) {
            events.add(name + ":" + i);
        }
    }

    public List<String> snapshot() {
        return List.copyOf(events);
    }
}
