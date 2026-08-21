# frozen_string_literal: true

# The base controller of the application. Every HTML controller
# inherits from it.
class ApplicationController < ActionController::Base
  protect_from_forgery with: :exception

  # Render the health check response.
  def health
    render json: { status: 'ok' }
  end
end
