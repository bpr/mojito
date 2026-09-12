# Trait inheritance / refinement `trait Bird(Animal):`.
trait Animal:
    def eat(self):
        ...

trait Bird(Animal):
    def fly(self):
        ...

@fieldwise_init
struct Sparrow(Bird):
    var age: Int
    def eat(self):
        pass
    def fly(self):
        pass

def flock[B: Bird](b: B):
    b.eat()
    b.fly()

def main():
    var s = Sparrow(2)
    flock(s)
    print(s.age)
