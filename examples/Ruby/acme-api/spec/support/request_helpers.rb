# frozen_string_literal: true

# Helpers for request specs.
module RequestHelpers
  # Parse the last response body as JSON.
  #
  # @return [Hash{String => Object}] the parsed body
  def json
    JSON.parse(last_response.body)
  end
end
