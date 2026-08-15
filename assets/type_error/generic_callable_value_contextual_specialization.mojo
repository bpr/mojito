# Current Mojo does not infer a generic function's specialization from a
# local callable annotation (`identity` does not become `def(Int) thin -> Int`
# contextually); only explicit specialization materializes a generic value.
# expect: does not infer a specialization
def identity[T: Copyable & Movable](value: T) -> T:
    return value

def main():
    var callback: def(Int) thin -> Int = identity
    print(callback(42))
