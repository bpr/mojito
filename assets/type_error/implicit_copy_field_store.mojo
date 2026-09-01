# expect: cannot be implicitly copied
# Storing an immutable parameter into a field implicitly copies it; a
# Copyable-only type needs an explicit `.copy()` (an immutable parameter
# cannot be transferred).
@fieldwise_init
struct Holder:
    var items: List[Int]

def store(mut holder: Holder, value: List[Int]):
    holder.items = value

def main():
    var holder = Holder(List[Int]())
    var values: List[Int] = [1]
    store(holder, values)
    print(len(holder.items))
