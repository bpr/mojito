# A bracket list leaving a parameter unbound is a generator, which has no
# run-time form: printing it is rejected, as it is not `Writable`.
# expect: an element of 'values' with type '__generator_type[b: Int] Int' does not conform to trait 'Writable'
comptime Add[a: Int, b: Int] = a + b


def main():
    print(Add[1, _])
