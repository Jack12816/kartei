# frozen_string_literal: true

VERSION = '1.0.0'

module Foo
  DEFAULT = 42

  class Bar
    attr_accessor :first, :second
    attr_reader :third
    attr_writer :fourth

    def initialize(first)
      @first = first
    end

    def self.build
      new(1)
    end

    class << self
      def cached
        @cached ||= build
      end
    end
  end

  module Util
    def self.helper; end
  end
end

class Foo::Baz
  def qux; end
end
