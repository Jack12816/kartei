# frozen_string_literal: true

module Metrics
  # The entry point of the gem: holds the configuration and the
  # collector registry.
  module Helper
    # Configure the gem.
    #
    # @yield [config] the configuration object
    def self.configure
      yield(config)
    end

    # The current configuration.
    #
    # @return [Metrics::Helper::Configuration] the configuration
    def self.config
      @config ||= Configuration.new
    end
  end
end
