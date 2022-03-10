pub trait MemoryUsage: Sized {
    fn estimated_heap_size(&self) -> usize;
    fn estimated_total_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.estimated_heap_size()
    }
}
