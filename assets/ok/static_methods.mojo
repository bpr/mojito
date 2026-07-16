struct Math:
    @staticmethod
    def twice(value: Int) -> Int:
        return value * 2

    @staticmethod
    def combine(left: Int, right: Int = 1) -> Int:
        return left + right

def main():
    print(Math.twice(6))
    print(Math.combine(4), Math.combine(right=3, left=4))
