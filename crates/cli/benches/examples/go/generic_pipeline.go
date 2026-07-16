package benchmark

import (
	"context"
	"errors"
	"fmt"
	"sync"
)

type Number interface {
	~int | ~int64 | ~float64
}

type Result[T any] struct {
	Value T
	Err   error
}

type Mapper[A, B any] interface {
	Map(context.Context, A) (B, error)
}

type MapperFunc[A, B any] func(context.Context, A) (B, error)

func (f MapperFunc[A, B]) Map(ctx context.Context, value A) (B, error) {
	return f(ctx, value)
}

type Pipeline[A, B any] struct {
	workers int
	mapper  Mapper[A, B]
}

func NewPipeline[A, B any](workers int, mapper Mapper[A, B]) *Pipeline[A, B] {
	if workers < 1 {
		workers = 1
	}
	return &Pipeline[A, B]{workers: workers, mapper: mapper}
}

func (p *Pipeline[A, B]) Run(ctx context.Context, input <-chan A) <-chan Result[B] {
	output := make(chan Result[B])
	var workers sync.WaitGroup
	workers.Add(p.workers)

	for worker := 0; worker < p.workers; worker++ {
		go func(id int) {
			defer workers.Done()
			for {
				select {
				case <-ctx.Done():
					return
				case value, ok := <-input:
					if !ok {
						return
					}
					mapped, err := p.mapper.Map(ctx, value)
					select {
					case output <- Result[B]{Value: mapped, Err: err}:
					case <-ctx.Done():
						return
					}
				}
			}
		}(worker)
	}

	go func() {
		workers.Wait()
		close(output)
	}()
	return output
}

func Sum[T Number](values []T) T {
	var total T
	for _, value := range values {
		total += value
	}
	return total
}

func Collect[T any](ctx context.Context, values <-chan Result[T]) ([]T, error) {
	result := make([]T, 0, 32)
	var failures []error
	for {
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case value, ok := <-values:
			if !ok {
				if len(failures) != 0 {
					return nil, errors.Join(failures...)
				}
				return result, nil
			}
			if value.Err != nil {
				failures = append(failures, value.Err)
				continue
			}
			result = append(result, value.Value)
		}
	}
}

func Describe(value any) string {
	switch value := value.(type) {
	case nil:
		return "nil"
	case string:
		return fmt.Sprintf("string(%q)", value)
	case fmt.Stringer:
		return value.String()
	case []byte:
		return fmt.Sprintf("bytes(%d)", len(value))
	default:
		return fmt.Sprintf("%T", value)
	}
}

func Example(ctx context.Context, input []int) ([]string, error) {
	values := make(chan int, len(input))
	for _, value := range input {
		values <- value
	}
	close(values)

	pipeline := NewPipeline(4, MapperFunc[int, string](
		func(ctx context.Context, value int) (string, error) {
			if value < 0 {
				return "", fmt.Errorf("negative value: %d", value)
			}
			return fmt.Sprintf("item-%04d", value), nil
		},
	))
	return Collect(ctx, pipeline.Run(ctx, values))
}
