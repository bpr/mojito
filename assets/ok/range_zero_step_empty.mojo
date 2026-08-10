def count_ctfe_iterations() -> Int:
    var count = 0
    for i in range(3, 0, 0):
        count += 1
    return count

comptime CTFE_COUNT = count_ctfe_iterations()

def main():
    var unrolled_count = 0
    comptime for i in range(0, 5, 0):
        unrolled_count += 1

    var runtime_count = 0
    for i in range(0, 5, 0):
        runtime_count += 1

    print(CTFE_COUNT, unrolled_count, runtime_count)
